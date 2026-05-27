#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NetworkMode {
    Testnet,
    Mainnet,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NetworkProfile {
    pub mode: NetworkMode,
    pub swarm_network_id: u64,
    pub wallet_chain_id: u64,
    pub base_symbol: &'static str,
    pub bzz_symbol: &'static str,
    pub bootnodes: &'static [&'static str],
}

pub(crate) const TESTNET_BOOTNODES: &[&str] = &[
    "/ip4/167.235.96.31/tcp/32535/tls/sni/167-235-96-31.k2k4r8n9x80nshvozftjmg4klymgjtdflwxiovfx63yc6917dlrteva4.libp2p.direct/ws/p2p/QmYkyg5ZU3DzxhqfGyLYLVbk9DMdBagxe9q1AmHKNgt8ps",
    "/ip4/49.12.172.37/tcp/32530/tls/sni/49-12-172-37.k2k4r8kibjadgpqco81quegou963p7lbcd9ti0bw8lrcc95ystm6by9d.libp2p.direct/ws/p2p/QmRHeoLCHjHoMur8PQpuV8acNJMmKPT61c3ZMLpTqY7og4",
    "/ip4/49.12.172.37/tcp/32533/tls/sni/49-12-172-37.k2k4r8pnvqpufzwaf4ic1o1fo0onfh4p9b37gp0rdxzdte2kcd7ewp4w.libp2p.direct/ws/p2p/QmfCwr7FVxbYz1GPQ2NN2r5iduXSQDLefqzkBAB9JfZYgF",
    "/ip4/167.235.96.31/tcp/32536/tls/sni/167-235-96-31.k2k4r8omeryzle2ywg941xs6vgwlq4cr0b3qe83ub7rn9n8ysmcwfqru.libp2p.direct/ws/p2p/QmcPvejw1r1BQ6aUuK6Y18mcLAcYyg9iEmiD1TRpyaox7s",
    "/ip4/49.12.172.37/tcp/32531/tls/sni/49-12-172-37.k2k4r8l8l5hzyp48440rjqlfdjpr03jfgioal93akbigy0tomtft4w44.libp2p.direct/ws/p2p/QmTFvqc5wMkbsXjqnTxQbVss5t8T1292BupJZ9VyU1GMRV",
    "/ip4/49.12.172.37/tcp/32532/tls/sni/49-12-172-37.k2k4r8pr3m3aug5nudg2y039qfj2gxw6wnlx0e0ghzxufcn38soyp9z4.libp2p.direct/ws/p2p/QmfSx1ujzboapD5h2CiqTJqUy46FeTDwXBszB3XUCfKEEj",
    "/ip4/167.235.96.31/tcp/32537/tls/sni/167-235-96-31.k2k4r8m6hc1wyzz789uubmz6cxmeuquzfi5b06zdh4l7e5ve199oay7j.libp2p.direct/ws/p2p/QmVoPN964YuoGpqc6BGJpLGmUn2goaqRm5vkCi5e7H9w98",
    "/ip4/167.235.96.31/tcp/32538/tls/sni/167-235-96-31.k2k4r8nqwaetj1eljpu4qzeebnnujzu997pdh1i1ia2dcpcjjv9gc1s0.libp2p.direct/ws/p2p/Qma2pmuYLCzcmsFHHLyWRPxxt7eN9MKqhnJaShKomn2zEK",
];

pub(crate) const MAINNET_BOOTNODES: &[&str] = &[
    "/ip4/139.84.229.70/tcp/1634/p2p/QmRa6rSrUWJ7s68MNmV94bo2KAa9pYcp6YbFLMHZ3r7n2M",
    "/ip4/135.181.84.53/tcp/1634/p2p/QmTxX73q8dDiVbmXU7GqMNwG3gWmjSFECuMoCsTW4xp6CK",
    "/ip4/159.223.6.181/tcp/1634/p2p/QmP9b7MxjyEfrJrch5jUThmuFaGzvUPpWEJewCpx5Ln6i8",
];

pub(crate) const TESTNET_PROFILE: NetworkProfile = NetworkProfile {
    mode: NetworkMode::Testnet,
    swarm_network_id: 10,
    wallet_chain_id: 11155111,
    base_symbol: "Sepolia ETH",
    bzz_symbol: "sBZZ",
    bootnodes: TESTNET_BOOTNODES,
};

pub(crate) const MAINNET_PROFILE: NetworkProfile = NetworkProfile {
    mode: NetworkMode::Mainnet,
    swarm_network_id: 1,
    wallet_chain_id: 100,
    base_symbol: "xDAI",
    bzz_symbol: "xBZZ",
    bootnodes: MAINNET_BOOTNODES,
};

pub(crate) fn profile_for_swarm_network_id(network_id: u64) -> Option<NetworkProfile> {
    match network_id {
        1 => Some(MAINNET_PROFILE),
        10 => Some(TESTNET_PROFILE),
        _ => None,
    }
}

pub(crate) fn profile_for_mode(mode: NetworkMode) -> NetworkProfile {
    match mode {
        NetworkMode::Testnet => TESTNET_PROFILE,
        NetworkMode::Mainnet => MAINNET_PROFILE,
    }
}

pub(crate) fn active_profile() -> NetworkProfile {
    if crate::is_mainnet() {
        MAINNET_PROFILE
    } else {
        TESTNET_PROFILE
    }
}

pub(crate) fn is_browser_dialable_underlay(address: &str) -> bool {
    address.contains("/ws/") || address.contains("/wss/")
}
