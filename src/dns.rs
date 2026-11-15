use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use hickory_resolver::{
    config::{LookupIpStrategy, NameServerConfigGroup, ResolverConfig},
    name_server::TokioConnectionProvider,
    Resolver, TokioResolver,
};
use tracing::{debug, warn};

const DNS_SERVERS: [Ipv4Addr; 3] = [
    Ipv4Addr::new(1, 1, 1, 1),
    Ipv4Addr::new(8, 8, 8, 8),
    Ipv4Addr::new(8, 8, 4, 4),
];
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);

/// Start every updater HTTP client with the same public DNS fallback policy.
pub fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().dns_resolver(PublicDnsResolver::new())
}

/// Resolve through Cloudflare, then each Google server, without using ISP DNS.
pub struct PublicDnsResolver {
    servers: Arc<[(SocketAddr, TokioResolver)]>,
    timeout: Duration,
}

impl Default for PublicDnsResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl PublicDnsResolver {
    pub fn new() -> Self {
        Self::with_servers(
            &DNS_SERVERS.map(|ip| SocketAddr::from((ip, 53))),
            LOOKUP_TIMEOUT,
        )
    }

    fn with_servers(addresses: &[SocketAddr], timeout: Duration) -> Self {
        let servers = addresses
            .iter()
            .map(|&address| {
                // Separate resolvers enforce provider order even for NXDOMAIN
                // responses, and keep each provider's DNS cache independent.
                let config = ResolverConfig::from_parts(
                    None,
                    Vec::new(),
                    NameServerConfigGroup::from_ips_clear(&[address.ip()], address.port(), true),
                );
                let mut builder =
                    Resolver::builder_with_config(config, TokioConnectionProvider::default());
                let options = builder.options_mut();
                options.timeout = timeout / 2;
                options.attempts = 1;
                options.ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
                options.try_tcp_on_error = true;
                (address, builder.build())
            })
            .collect();

        Self { servers, timeout }
    }
}

impl reqwest::dns::Resolve for PublicDnsResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let servers = self.servers.clone();
        let timeout = self.timeout;
        Box::pin(async move {
            let mut failures = Vec::new();
            for (address, resolver) in servers.iter() {
                // Bound the entire attempt, including A/AAAA lookups and TCP
                // retries, so an unresponsive server cannot stall all fallbacks.
                let error =
                    match tokio::time::timeout(timeout, resolver.lookup_ip(name.as_str())).await {
                        Ok(Ok(lookup)) if lookup.iter().next().is_some() => {
                            debug!(host = name.as_str(), server = %address, "DNS lookup succeeded");
                            let addrs: reqwest::dns::Addrs =
                                Box::new(lookup.into_iter().map(|ip| SocketAddr::new(ip, 0)));
                            return Ok(addrs);
                        }
                        Ok(Ok(_)) => "no IP addresses returned".to_owned(),
                        Ok(Err(error)) => error.to_string(),
                        Err(_) => format!("timed out after {timeout:?}"),
                    };
                warn!(host = name.as_str(), server = %address, %error, "DNS lookup failed");
                failures.push(format!("{address}: {error}"));
            }

            Err(io::Error::other(format!(
                "DNS lookup failed for {} using all public resolvers ({})",
                name.as_str(),
                failures.join("; ")
            ))
            .into())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::Ipv6Addr, sync::Mutex};

    use hickory_resolver::proto::{
        op::{Message, MessageType, ResponseCode},
        rr::{
            rdata::{A, AAAA},
            RData, Record, RecordType,
        },
    };
    use reqwest::dns::Resolve;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, UdpSocket},
        task::JoinHandle,
    };

    #[derive(Clone, Copy)]
    enum Reply {
        Addresses,
        Ipv6Only,
        Error(ResponseCode),
        Silent,
    }

    struct DnsServer {
        address: SocketAddr,
        task: JoinHandle<()>,
    }

    impl Drop for DnsServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    impl DnsServer {
        async fn start(index: usize, reply: Reply, calls: Arc<Mutex<Vec<usize>>>) -> Self {
            let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let address = socket.local_addr().unwrap();
            let task = tokio::spawn(async move {
                let mut buffer = [0; 4096];
                loop {
                    let (length, peer) = socket.recv_from(&mut buffer).await.unwrap();
                    let request = Message::from_vec(&buffer[..length]).unwrap();
                    calls.lock().unwrap().push(index);
                    if matches!(reply, Reply::Silent) {
                        continue;
                    }

                    let mut response = Message::new();
                    response
                        .set_id(request.id())
                        .set_message_type(MessageType::Response)
                        .set_authoritative(true)
                        .set_recursion_desired(request.recursion_desired())
                        .set_recursion_available(true)
                        .add_queries(request.queries().iter().cloned());
                    if let Reply::Error(code) = reply {
                        response.set_response_code(code);
                    } else {
                        for query in request.queries() {
                            let records = match query.query_type() {
                                RecordType::A if matches!(reply, Reply::Addresses) => vec![
                                    RData::A(A(Ipv4Addr::LOCALHOST)),
                                    RData::A(A(Ipv4Addr::new(127, 0, 0, 2))),
                                ],
                                RecordType::AAAA => vec![RData::AAAA(AAAA(Ipv6Addr::LOCALHOST))],
                                _ => Vec::new(),
                            };
                            for data in records {
                                response.add_answer(Record::from_rdata(
                                    query.name().clone(),
                                    60,
                                    data,
                                ));
                            }
                        }
                    }
                    socket
                        .send_to(&response.to_vec().unwrap(), peer)
                        .await
                        .unwrap();
                }
            });
            Self { address, task }
        }
    }

    async fn servers(
        replies: &[Reply],
    ) -> (Vec<DnsServer>, PublicDnsResolver, Arc<Mutex<Vec<usize>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut servers = Vec::new();
        for (index, &reply) in replies.iter().enumerate() {
            servers.push(DnsServer::start(index, reply, calls.clone()).await);
        }
        let addresses: Vec<_> = servers.iter().map(|server| server.address).collect();
        let resolver = PublicDnsResolver::with_servers(&addresses, Duration::from_millis(200));
        (servers, resolver, calls)
    }

    fn visited(calls: &Mutex<Vec<usize>>) -> Vec<usize> {
        let mut order = calls.lock().unwrap().clone();
        order.dedup();
        order
    }

    #[tokio::test]
    async fn primary_success_is_cached_and_does_not_query_fallbacks() {
        let (_servers, resolver, calls) = servers(&[Reply::Addresses; 3]).await;
        for _ in 0..2 {
            let addresses: Vec<_> = resolver
                .resolve("updater.test".parse().unwrap())
                .await
                .unwrap()
                .collect();
            assert_eq!(addresses.len(), 3);
            assert!(addresses.iter().all(|address| address.port() == 0));
            assert!(addresses.contains(&SocketAddr::from((Ipv4Addr::LOCALHOST, 0))));
            assert!(addresses.contains(&SocketAddr::from((Ipv6Addr::LOCALHOST, 0))));
        }
        assert_eq!(*calls.lock().unwrap(), vec![0, 0]); // one A and one AAAA query
    }

    #[tokio::test]
    async fn falls_back_in_order_after_server_failure_and_authoritative_nxdomain() {
        let (_servers, resolver, calls) = servers(&[
            Reply::Error(ResponseCode::ServFail),
            Reply::Error(ResponseCode::NXDomain),
            Reply::Addresses,
        ])
        .await;
        let addresses: Vec<_> = resolver
            .resolve("updater.test".parse().unwrap())
            .await
            .unwrap()
            .collect();
        assert_eq!(addresses.len(), 3);
        assert_eq!(visited(&calls), vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn unresponsive_primary_is_bounded_and_second_server_succeeds() {
        let (_servers, resolver, calls) =
            servers(&[Reply::Silent, Reply::Addresses, Reply::Addresses]).await;
        let addresses = tokio::time::timeout(
            Duration::from_secs(2),
            resolver.resolve("updater.test".parse().unwrap()),
        )
        .await
        .expect("DNS fallback must not hang")
        .unwrap();
        assert_eq!(addresses.count(), 3);
        assert_eq!(visited(&calls), vec![0, 1]);
    }

    #[tokio::test]
    async fn empty_answers_fall_back_and_ipv6_only_hosts_work() {
        let (_servers, resolver, calls) = servers(&[
            Reply::Error(ResponseCode::NoError),
            Reply::Ipv6Only,
            Reply::Addresses,
        ])
        .await;
        let addresses: Vec<_> = resolver
            .resolve("updater.test".parse().unwrap())
            .await
            .unwrap()
            .collect();
        assert_eq!(addresses, vec![SocketAddr::from((Ipv6Addr::LOCALHOST, 0))]);
        assert_eq!(visited(&calls), vec![0, 1]);
    }

    #[tokio::test]
    async fn all_failures_report_host_and_every_attempted_server() {
        let (servers, resolver, calls) = servers(&[
            Reply::Error(ResponseCode::NXDomain),
            Reply::Error(ResponseCode::Refused),
            Reply::Error(ResponseCode::NoError),
        ])
        .await;
        let error = resolver
            .resolve("updater.test".parse().unwrap())
            .await
            .err()
            .expect("all servers failed")
            .to_string();
        assert!(error.contains("updater.test"));
        for server in &servers {
            assert!(error.contains(&server.address.to_string()), "{error}");
        }
        assert_eq!(visited(&calls), vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn all_unresponsive_servers_finish_with_an_error() {
        let (_servers, resolver, calls) = servers(&[Reply::Silent; 3]).await;
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            resolver.resolve("updater.test".parse().unwrap()),
        )
        .await
        .expect("all DNS attempts must be bounded");
        assert!(result.is_err());
        assert_eq!(visited(&calls), vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn reqwest_uses_fallback_and_preserves_hostname_and_url_port() {
        let (_servers, resolver, calls) =
            servers(&[Reply::Error(ResponseCode::ServFail), Reply::Addresses]).await;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
                assert!(request.len() < 8192);
            }
            let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
            assert!(
                request.contains(&format!("host: updater.test:{port}\r\n")),
                "{request}"
            );
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        });
        let client = client_builder()
            .dns_resolver(resolver)
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let body = client
            .get(format!("http://updater.test:{port}/manifest.json"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "ok");
        server.await.unwrap();
        assert_eq!(visited(&calls), vec![0, 1]);
    }
}
