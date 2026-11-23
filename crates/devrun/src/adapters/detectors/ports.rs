use devrun_domain::Port;

pub fn ports_in(text: &str) -> Vec<Port> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut ports = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let next = tokens.get(index + 1).copied();
        let raw = match *token {
            "-p" | "--port" => next,
            _ => token
                .strip_prefix("--port=")
                .or_else(|| token.strip_prefix("PORT=")),
        };
        if let Some(port) = raw.and_then(parse_port)
            && !ports.contains(&port)
        {
            ports.push(port);
        }
    }
    ports
}

pub fn parse_port(raw: &str) -> Option<Port> {
    raw.parse::<u16>()
        .ok()
        .and_then(|value| Port::new(value).ok())
}
