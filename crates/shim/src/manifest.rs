//! Translates a [`DecoderManifest`] from the IPC layer into the JSON
//! shape WaveCrux's open-core decoder loader expects.
//!
//! WaveCrux's loader reads `WcDecoderDef.manifest_json` and maps it into
//! a `DecoderDefinition` (signals, parameters, category). The shape is
//! documented in the WaveCrux `wavecrux_decoder.h` header.

use serde_json::{json, Value};

use wavecrux_sigrok_bridge_ipc::DecoderManifest;

/// Build the WaveCrux-side manifest JSON for one bridged decoder.
pub(crate) fn build_manifest_json(m: &DecoderManifest) -> String {
    // GPL notice is embedded in the description so it surfaces in the
    // WaveCrux UI's decoder picker — invariant 4 in CLAUDE.md.
    let description = format!(
        "{}\n\nProvided by libsigrokdecode (GPLv3+) via the \
         WaveCrux SigRok bridge plugin. https://sigrok.org",
        m.description
    );
    let signals: Vec<Value> = m
        .channels
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "description": c.description,
                "optional": !c.required,
                "bit_width": 1,
            })
        })
        .collect();
    let parameters: Vec<Value> = m
        .options
        .iter()
        .map(|o| {
            json!({
                "name": o.name,
                "description": o.description,
                "kind": format!("{:?}", o.kind).to_lowercase(),
                "default": o.default,
                "choices": o.choices,
            })
        })
        .collect();
    let manifest = json!({
        "id": m.id,
        "display_name": m.display_name,
        "description": description,
        "category": "user",
        "license": "GPL-3.0-or-later",
        "source": "sigrok",
        "signals": signals,
        "parameters": parameters,
        "annotations": m.annotations.iter().map(|a| json!({
            "id": a.id,
            "description": a.description,
        })).collect::<Vec<_>>(),
        "tags": m.tags,
    });
    serde_json::to_string(&manifest).expect("manifest serialization")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wavecrux_sigrok_bridge_ipc::{
        DecoderAnnotationClass, DecoderChannel, DecoderOption, OptionKind,
    };

    #[test]
    fn manifest_includes_gpl_notice() {
        let m = DecoderManifest {
            id: "sigrok.onewire".into(),
            display_name: "1-Wire".into(),
            description: "Maxim 1-Wire bus".into(),
            channels: vec![DecoderChannel {
                name: "data".into(),
                description: "Data line".into(),
                required: true,
            }],
            options: vec![],
            annotations: vec![DecoderAnnotationClass {
                id: "rom".into(),
                description: "ROM commands".into(),
            }],
            tags: vec!["embedded".into()],
        };
        let s = build_manifest_json(&m);
        assert!(s.contains("GPLv3+"));
        assert!(s.contains("libsigrokdecode"));
        assert!(s.contains("sigrok.onewire"));
    }

    #[test]
    fn manifest_serializes_options() {
        let m = DecoderManifest {
            id: "sigrok.uart".into(),
            display_name: "UART".into(),
            description: "Asynchronous serial".into(),
            channels: vec![DecoderChannel {
                name: "rx".into(),
                description: "Receive line".into(),
                required: true,
            }],
            options: vec![DecoderOption {
                name: "baudrate".into(),
                description: "Baud rate".into(),
                kind: OptionKind::Int,
                default: serde_json::json!(115_200),
                choices: vec![],
            }],
            annotations: vec![],
            tags: vec![],
        };
        let s = build_manifest_json(&m);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let params = v["parameters"].as_array().unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0]["name"], "baudrate");
        assert_eq!(params[0]["kind"], "int");
    }
}
