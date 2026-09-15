//! Parse PipeWire's native object catalog. Runtime object IDs are deliberately
//! excluded from endpoint identity; node.name is the persistent routing key.
use rshare_core::network_audio::*;
pub fn catalog(json: &[u8]) -> Result<Vec<Endpoint>, String> {
    if json.len() > 16 * 1024 * 1024 {
        return Err("PipeWire catalog exceeds 16 MiB".into());
    }
    let nodes: Vec<serde_json::Value> = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let mut endpoints = vec![];
    for node in nodes {
        if node["type"].as_str() != Some("PipeWire:Interface:Node") {
            continue;
        }
        let props = &node["info"]["props"];
        let direction = match props["media.class"].as_str() {
            Some("Audio/Source") => Direction::Input,
            Some("Audio/Sink") => Direction::Output,
            _ => continue,
        };
        let Some(id) = props["node.name"].as_str() else {
            continue;
        };
        let name = props["node.description"].as_str().unwrap_or(id);
        let channels = props["audio.channels"].as_u64().or_else(|| {
            props["audio.channels"]
                .as_str()
                .and_then(|s| s.parse().ok())
        });
        let Some(channels) = channels.filter(|c| *c > 0 && *c <= 255) else {
            continue;
        };
        let virtual_device = id.starts_with("rshare-audio-");
        // PipeWire graph adaptation accepts these rates; physical sample-rate
        // support remains a separate stream-negotiation result.
        endpoints.push(Endpoint {
            id: id.into(),
            name: name.into(),
            direction,
            channels: channels as u8,
            sample_rates: vec![48000, 96000],
            virtual_device,
            available: node["info"]["state"].as_str() != Some("error"),
        });
    }
    if endpoints.len() > MAX_ENDPOINTS {
        return Err("too many PipeWire endpoints".into());
    }
    for endpoint in &endpoints {
        endpoint.validate().map_err(|e| e.to_string())?;
    }
    Ok(endpoints)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_identity_does_not_depend_on_pipewire_object_id() {
        let first=br#"[{"id":42,"type":"PipeWire:Interface:Node","info":{"state":"suspended","props":{"node.name":"alsa_input.usb-interface","node.description":"Studio","audio.channels":8,"media.class":"Audio/Source"}}}]"#;
        let a = catalog(first).unwrap();
        let modified = String::from_utf8(first.to_vec())
            .unwrap()
            .replace("42", "99");
        let b = catalog(modified.as_bytes()).unwrap();
        assert_eq!(a, b);
        assert_eq!(a[0].channels, 8);
    }
    #[test]
    fn virtual_rshare_nodes_are_identified_for_export_suppression() {
        let nodes=catalog(br#"[{"type":"PipeWire:Interface:Node","info":{"props":{"node.name":"rshare-audio-a","audio.channels":"2","media.class":"Audio/Sink"}}}]"#).unwrap();
        assert!(nodes[0].virtual_device);
    }
}
