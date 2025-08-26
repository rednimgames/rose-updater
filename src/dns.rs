use std::{net::SocketAddr, sync::Arc};

use hickory_resolver::{
    config::ResolverConfig,
    lookup_ip::LookupIpIntoIter,
    name_server::{GenericConnector, TokioConnectionProvider},
    proto::runtime::TokioRuntimeProvider,
    Resolver,
};

pub struct CloudflareResolver {
    resolver: Arc<Resolver<GenericConnector<TokioRuntimeProvider>>>,
}

impl CloudflareResolver {
    pub fn new() -> Self {
        let resolver = Resolver::builder_with_config(
            ResolverConfig::cloudflare(),
            TokioConnectionProvider::default(),
        )
        .build();

        Self {
            resolver: Arc::new(resolver),
        }
    }
}

impl reqwest::dns::Resolve for CloudflareResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let resolver = self.resolver.clone();
        Box::pin(async move {
            let lookup = resolver.lookup_ip(name.as_str()).await?;
            let addrs: reqwest::dns::Addrs = Box::new(HickoryAddrs {
                iter: lookup.into_iter(),
            });
            Ok(addrs)
        })
    }
}

struct HickoryAddrs {
    pub iter: LookupIpIntoIter,
}

impl Iterator for HickoryAddrs {
    type Item = SocketAddr;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|ip_addr| SocketAddr::new(ip_addr, 0))
    }
}
