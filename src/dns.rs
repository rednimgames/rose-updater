use std::{net::SocketAddr, sync::Arc};

use hickory_resolver::{
    config::ResolverConfig,
    lookup_ip::LookupIpIntoIter,
    name_server::{GenericConnector, TokioConnectionProvider},
    proto::runtime::TokioRuntimeProvider,
    Resolver,
};
use tracing::warn;

type HickoryResolver = Resolver<GenericConnector<TokioRuntimeProvider>>;

pub struct CloudflareResolver {
    cloudflare: Arc<HickoryResolver>,
    system: Arc<HickoryResolver>,
}

impl CloudflareResolver {
    pub fn new() -> Self {
        let cloudflare = Resolver::builder_with_config(
            ResolverConfig::cloudflare(),
            TokioConnectionProvider::default(),
        )
        .build();

        let system = Resolver::builder_with_config(
            ResolverConfig::default(),
            TokioConnectionProvider::default(),
        )
        .build();

        Self {
            cloudflare: Arc::new(cloudflare),
            system: Arc::new(system),
        }
    }
}

impl reqwest::dns::Resolve for CloudflareResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let cloudflare = self.cloudflare.clone();
        let system = self.system.clone();
        Box::pin(async move {
            match cloudflare.lookup_ip(name.as_str()).await {
                Ok(lookup) => {
                    let addrs: reqwest::dns::Addrs = Box::new(HickoryAddrs {
                        iter: lookup.into_iter(),
                    });
                    Ok(addrs)
                }
                Err(e) => {
                    warn!(
                        "Cloudflare DNS resolution failed for {}, falling back to system DNS: {}",
                        name.as_str(),
                        e
                    );
                    let lookup = system.lookup_ip(name.as_str()).await?;
                    let addrs: reqwest::dns::Addrs = Box::new(HickoryAddrs {
                        iter: lookup.into_iter(),
                    });
                    Ok(addrs)
                }
            }
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
