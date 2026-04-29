use rand::Rng;

pub const UA_LINUX: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/60.0.3112.90 Safari/537.36";
pub const UA_PC: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36 Edg/124.0.0.0";
pub const UA_MOBILE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4.1 Mobile/15E148 Safari/604.1";

#[derive(Clone, Copy, Debug)]
pub enum UaKind {
    Pc,
    Mobile,
    Linux,
    Any,
}

pub fn pick_user_agent(kind: UaKind) -> &'static str {
    match kind {
        UaKind::Pc => UA_PC,
        UaKind::Mobile => UA_MOBILE,
        UaKind::Linux => UA_LINUX,
        UaKind::Any => {
            let mut rng = rand::rng();
            if rng.random::<bool>() {
                UA_PC
            } else {
                UA_MOBILE
            }
        }
    }
}
