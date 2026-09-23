fn main() {
    println!("harness-demo protocol {}", harness::protocol::VERSION);
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_round_trip() {
        let v = harness::protocol::ProtocolVersion(harness::protocol::VERSION);
        let wire = serde_json::to_string(&v).expect("serialize");
        assert_eq!(
            serde_json::from_str::<harness::protocol::ProtocolVersion>(&wire)
                .expect("deserialize"),
            v
        );
    }
}
