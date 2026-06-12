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
    "/dns4/116-202-168-171.k2k4r8n5xpu0545g9br7x5kts9hjafkng0absz38588ex6dtnsos4ezw.libp2p.direct/tcp/31714/tls/ws/p2p/QmYTdpEnzXh6GMZde87sBb5A4raWUzqTMSpt4BDtLqTdhV",
    "/dns4/116-202-192-234.k2k4r8nl2ayrj26qzy3dcqb0m5pnxntzzfs0yvxrkjrsjww1hdy87nnc.libp2p.direct/tcp/31605/tls/ws/p2p/QmZbSwqnp5jbo5E6CH4HYp5vpGSBLYvidQXwdPq1iH2LmD",
    "/dns4/116-202-192-234.k2k4r8pq1dwfxn825fzv74bw7br794b4xjzqr2bkhe54vb4bxx4i7g0d.libp2p.direct/tcp/31715/tls/ws/p2p/QmfNL56LhiJiVkYYBkj4Pm8vFXiSxnabWVHzf9yF155VVe",
    "/dns4/116-202-168-171.k2k4r8njbl5qb7ndt5uq32tmqg35ffyvf66e4q7uzr6ojnnkystmgyyr.libp2p.direct/tcp/31622/tls/ws/p2p/QmZTsMFsiKY91yBxdDEy8eWDhcqaM2acuDx1FEjvVbLWAE",
    "/dns4/116-202-192-234.k2k4r8ozmsoono69vklz45suq3oih3fyuxchvyo9xl0m8bojhuzwkiv9.libp2p.direct/tcp/31651/tls/ws/p2p/QmdPTBPu4uMxLbi7xBixrPFL7rxX6vi6R3rCEEa9koy7WU",
    "/dns4/116-202-193-61.k2k4r8om2izjmnhe13zrt9dub1pmi4qbhltut4rbjp9b83npyy30xpep.libp2p.direct/tcp/31746/tls/ws/p2p/QmcNSns3B2iEn2rChwULvjUtJx6f2twAhG6A4ZMXzYXR8k",
    "/dns4/116-202-168-171.k2k4r8p6sege0qz7lz794p4e5z5n13uk18y0ceqk4oilnfs1kxnyriij.libp2p.direct/tcp/31722/tls/ws/p2p/Qmdvanh2EEkZVFaYfWJUXuJddeTHifboMkx986Kvknamn2",
    "/dns4/116-202-192-234.k2k4r8jts4h90rh3grxnnkgs3ugjuqrcu8ko4r8ihv9njzjfpeoos998.libp2p.direct/tcp/31695/tls/ws/p2p/QmPStmpKAJfi7NK44pE8yTi9wFx2AJe4uSBK2ffEeiiipT",
    "/dns4/116-202-193-61.k2k4r8n2w8mu3lkhzz62wwktr7buqe1diku3s0amnjmn8uzk2iocn63p.libp2p.direct/tcp/31646/tls/ws/p2p/QmYEQTiT99tZaWFU3HpqhCipSa5bnUNN1ddY6cFSGXvdpg",
    "/dns4/109-205-181-149.k2k4r8pr4vxpclfexnh8xf2a5e0gq4jfcbp4vm51x4rrek13s75d9uay.libp2p.direct/tcp/31634/tls/ws/p2p/QmfT6wcfFWcDQShQMz2A6juFxVPArpdJGgRBJCS8WWTGdf",
    "/dns4/116-202-168-171.k2k4r8n28iiq6xihzaghnt48gcjx8kiydw265tbdu2xzmgssghkn12r4.libp2p.direct/tcp/31717/tls/ws/p2p/QmYBYAWuL8qjt6QnJBP7vuWWso8jK8ixqMobabKAyMKnsy",
    "/dns4/116-202-193-61.k2k4r8nrp1kfmdn45tnzsb6n5inhhh55n9up7c4lqeacf7c4sc855gpc.libp2p.direct/tcp/31662/tls/ws/p2p/Qma6JKYZDoB3rYp6YGNffgYENnuRF37gMmE36RZTZMYUQw",
    "/dns4/116-202-193-61.k2k4r8p3si6gghrysmf8m0l62nd13qp1ktb9a7isqv21jnhhlbmal82s.libp2p.direct/tcp/31621/tls/ws/p2p/QmdhYWyqQki81Sx6tCwv7xUG8Fy4si7ha7PaQi5bjS2uLF",
    "/dns4/116-202-192-234.k2k4r8mtdthysdadymk7lk2arlhilp0km54i1kpl24ihn5yszjz26u7l.libp2p.direct/tcp/31730/tls/ws/p2p/QmXX2MskEErqnYj2B6tV5PU2g71itgsHitZtELNeCbPBpt",
    "/dns4/116-202-168-171.k2k4r8lk1fabh2rtsork1w9gl6b1s61980q7fvw7s5ik87uet302u0ut.libp2p.direct/tcp/31662/tls/ws/p2p/QmU7kYV8hU9Dsn9LWCRMyBcFGcA4Jg6WNzP4p2uPwYRCng",
    "/dns4/116-202-168-171.k2k4r8k84nafyf8l74d5xcnupgf2pavnt5by4pouewnmleyej1348rln.libp2p.direct/tcp/31675/tls/ws/p2p/QmQXKCEHNtzSL8mUCWgJM9tL84xzHC9E1stFQbBSxJFFXQ",
    "/dns4/116-202-168-171.k2k4r8o02qizfbuai9ev8r01dmpzx884pd772dv0bzwdsll217i7hw9c.libp2p.direct/tcp/31604/tls/ws/p2p/QmaikxQ7QpcdiSjPxVa45A25j76anWhNWSxHuQdz7HHiZD",
    "/dns4/116-202-192-234.k2k4r8neldfqh4472w327ee1gyquz6o6ouj142k5jxb18ddl17p57xi3.libp2p.direct/tcp/31678/tls/ws/p2p/QmZ7JH8NqktQ5RRxU3SBxZjDJL5oYdmEmjsMpxNLEAzPsQ",
    "/dns4/116-202-192-234.k2k4r8l8ea3ujg5mn89q5tiv0pa6js1dowjbox8x7kwjhgt4ekujbhnn.libp2p.direct/tcp/31693/tls/ws/p2p/QmTF6ftLtQyHQ8rKAoB6VXoCyi6YBSLzfjgJjvopNgtoQn",
    "/dns4/116-202-193-61.k2k4r8my0leu54yjxkgpjml6o6p2qptzadfq57sulizcsr94gha6421x.libp2p.direct/tcp/31649/tls/ws/p2p/QmXsBKshma7kqgoveCJVUUeHj1p7uJiWgRAE5atckhu23E",
    "/dns4/116-202-192-234.k2k4r8p535kzd6cptzebk7aq6aj9878d6m261vr9qthcdshwh2tauxx2.libp2p.direct/tcp/31638/tls/ws/p2p/QmdoBVsVHhNNPHtPCGxJzEGfxB6q6GvAPQ68y85nUkfFGR",
    "/dns4/116-202-168-171.k2k4r8lxavgwzcweheap3x2xc8ihfx5jieota685k7xuj8b8u2e7u8pz.libp2p.direct/tcp/31670/tls/ws/p2p/QmV7T6ND9YPHDW7fYcHhGEnp78huPV1qWaHJJj4p4vRAn2",
    "/dns4/116-202-193-61.k2k4r8mw4vpnhjj4vvwocu39xiz0i9jr1cqm28kxziu5ts0fu0zeccts.libp2p.direct/tcp/31669/tls/ws/p2p/QmXizhphrRLrCPMzRCPup66W7YsVsT2dfRoNSPqTSEkVwV",
    "/dns4/116-202-193-61.k2k4r8jwcmtwx639qr65e0i34jczguaodhds9dfyhdsiir1tnrl38csp.libp2p.direct/tcp/31618/tls/ws/p2p/QmPe5CGFQCQ7RRBTdHJMV6wFkBuPizUVBBLCa6Nhsfn88G",
    "/dns4/116-202-192-234.k2k4r8n7u4perykkrmcaavbncrcloyoj6mzfy3fadoob96rhjxgdccwv.libp2p.direct/tcp/31661/tls/ws/p2p/QmYbuLZJohB3KN5kdNP2M1sMRYiwb6oZifqSY6exQYMpEa",
    "/dns4/116-202-193-61.k2k4r8kvj8cntin9m1qegjfi5ta77alvt40likeeu5co3hkete1h3cgx.libp2p.direct/tcp/31718/tls/ws/p2p/QmSH94RaBP1bcWW7N1Jr1K3yrZvLof34SvMvEm9PoLvSja",
    "/dns4/116-202-192-234.k2k4r8o8wgucd4mhfwh9vap6fvq4t11zi7ffskhoao68o2pdb82499t8.libp2p.direct/tcp/31716/tls/ws/p2p/QmbPA183DjEzPvP77uF1exYb7L14BQMK9zvUrjUuc9ZVr3",
    "/dns4/116-202-192-234.k2k4r8pgx4lhgi918dt1zm5w11j9pacgdhljot7i867cphdg85i7sews.libp2p.direct/tcp/31726/tls/ws/p2p/QmegfEb22QAsYqDSME1Eh8myD1QqdXKgC5GdocjhX8nnDy",
    "/dns4/116-202-192-234.k2k4r8k9w5dm2uyz0h89679s1tu7dulkwkqz86mo59ejix2kvl4egrck.libp2p.direct/tcp/31604/tls/ws/p2p/QmQezJBrUpeBUpGtEQaBs3FDU6z6eak2iBKQtMmm76QKKq",
    "/dns4/116-202-193-61.k2k4r8midirfrz2jp4wgud1zb9gybmpo53uwwuv8evftiy7ydt11k7s5.libp2p.direct/tcp/31709/tls/ws/p2p/QmWh8cpg3h2zFWjwD13WiAToe8KJ2KfagpnL8HaNBTSneg",
    "/dns4/116-202-168-171.k2k4r8n0xq0u5gw4o53cgpnlshsfdz0j3jd228utq2tzrdrbkt6wtnxb.libp2p.direct/tcp/31721/tls/ws/p2p/QmY5tC7AgTHwi8tqvhpNUyYuhd9t1hMB6E8HaxzKNiSsk2",
    "/dns4/116-202-193-61.k2k4r8n82qak1mp0aiq1dl0dvusi7gmr38xd20aqaj06h3cgg14b4xhm.libp2p.direct/tcp/31619/tls/ws/p2p/QmYcwcceFuv7MDHe7Jy89TytDT68quCJVYBEvRrotmNjD3",
    "/dns4/116-202-192-234.k2k4r8jxnhmubakk8wwdil3r9loznj5bwhp8hjaxwrvpcefxyb3ru6ny.libp2p.direct/tcp/31673/tls/ws/p2p/QmPjjcik6VQVC8jWDFC2bbmPPbq6sSpdcwd5LfdUVB7QAh",
    "/dns4/116-202-192-234.k2k4r8n4l95gce13dnur955jnxrh5m43kosfy3ikyg51un8a3ym88pq6.libp2p.direct/tcp/31646/tls/ws/p2p/QmYMn8BX75ond7P8EyzpvUddbY8RvcqWJwtsfT65oks13B",
    "/dns4/116-202-192-234.k2k4r8oc04f2q7ev9k0zc1g2n28brrxxhoswvl2vge585ejg3ung01xq.libp2p.direct/tcp/31697/tls/ws/p2p/Qmbcecj3ryfgV5fmdEMbEWBCaA91CuoqRkoXWpHQzMhnjP",
    "/dns4/116-202-193-61.k2k4r8pm3arw0kiko61ef9krj1gefyjl9r4mrakbtuieevem5ef90g4c.libp2p.direct/tcp/31702/tls/ws/p2p/Qmf5A9qTgzF3SYxjWfFVcLAZ2BzisrrvTRLYepErxHkHj1",
    "/dns4/116-202-193-61.k2k4r8k8uivsy3mmm29sx3nv84pakevgt6eyhmnso9a3r0rax8otwbwl.libp2p.direct/tcp/31650/tls/ws/p2p/QmQaSaNd6WX5P14mR6FvJ22yUbEoUCWi4YvQw6wZkuHTuv",
    "/dns4/116-202-168-171.k2k4r8o70glvffagstyqo01r6rvmsy4af0bolwumj9iutvwvbmfzuep9.libp2p.direct/tcp/31624/tls/ws/p2p/QmbEwL6SGFUq7JKmg2oKpuMcW6fk3rPBg2ykAzFPypQ9SU",
    "/dns4/116-202-192-234.k2k4r8lv42x19xqcbq32vyze8tiab1yqorqhepgoh7udm43s8c2kv93b.libp2p.direct/tcp/31687/tls/ws/p2p/QmUwvpDZ4JnPjjccg4JmVnMevRY6E8UBNV9QffZyJzVxeS",
    "/dns4/116-202-193-61.k2k4r8k28rwd6ea132gdilz0kp7qncbzdnx1llic2nbuvd48m9lgaeur.libp2p.direct/tcp/31748/tls/ws/p2p/QmQ5i8tNdxEneWsVTTiWpzJAvxQh8smaqDhEtYfbJTEoR4",
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
