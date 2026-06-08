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
    "/ip4/109.205.181.149/tcp/31644/tls/sni/109-205-181-149.k2k4r8lwucsb3m1gkhs1i5so1t1490mrd33il7griouxnwbwdlv2fecb.libp2p.direct/ws/p2p/QmV5TJpe73ri1DHU9nYajmUsoXYzhNrXXUZcyGDnD1LHsG",
    "/ip4/109.205.181.149/tcp/31646/tls/sni/109-205-181-149.k2k4r8lo6z0cqtvt0ckl8lbwjd2hxf2oso1cqb636xyag9dhqy1gue8d.libp2p.direct/ws/p2p/QmURpkuZxhNMe83dsyCGrnxqBGrM99aaVA3iiALj6aTuJc",
    "/ip4/109.205.181.149/tcp/31637/tls/sni/109-205-181-149.k2k4r8nclu1rjojzhrhpn973uo4p230l1i8kx4p5fsv6dhxiklbdui7h.libp2p.direct/ws/p2p/QmYxerDASAhBU5iiM7VvVaHCpQBtVxEwtYfe1ZyQj9nFrL",
    "/ip4/109.205.181.149/tcp/31651/tls/sni/109-205-181-149.k2k4r8p4cpdfyxi3ccgwbw4wy9zzsjzx5ufjt0wnswofp5dtlulg51h8.libp2p.direct/ws/p2p/Qmdjz6vCCjYCpfw5CZM5fK3aG1QAmQWc5hvQSq5oANt8wV",
    "/ip4/109.205.181.149/tcp/31650/tls/sni/109-205-181-149.k2k4r8o4bzy1a2swvot681mtbf5grhnriujsl65wb65qacvaixvgkdn0.libp2p.direct/ws/p2p/Qmb3HCb53pivCSTbEq85mAfPwvhbiuH1oHVDPFhv5rMx2f",
    "/ip4/159.223.6.181/tcp/1635/tls/sni/159-223-6-181.k2k4r8jpsxkovxvl3sf2u8bkg59z4mxmgfhs9r7ir8mcwduokkrcmku5.libp2p.direct/ws/p2p/QmP9b7MxjyEfrJrch5jUThmuFaGzvUPpWEJewCpx5Ln6i8",
    "/ip4/126.108.216.112/tcp/1635/tls/sni/126-108-216-112.k2k4r8novlzzfbrx3cub8u6iyuccliun26jfu4rd9jxbp3osil4fyak8.libp2p.direct/ws/p2p/QmZt3Migk2xknRxt3T2KdG53LiL518qELobxsZXEYh5nuV",
    "/ip4/216.238.102.247/tcp/1635/tls/sni/216-238-102-247.k2k4r8k8ccrb8q7ccsm6qfhq7s8g9ycdc7fe4abv8fnjs56mshu4ri7x.libp2p.direct/ws/p2p/QmQYFDafiKuWUDknur8VcTUVgJgNxJevLxzYRKDKKvvv1r",
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
