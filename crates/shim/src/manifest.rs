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
    // WaveCrux reads required channels from `signals` and optional ones
    // from `optional_signals`; a per-entry flag is not part of its
    // manifest format.
    let signal_entries = |required: bool| -> Vec<Value> {
        m.channels
            .iter()
            .filter(|c| c.required == required)
            .map(|c| {
                json!({
                    "name": c.name,
                    "description": c.description,
                    "bit_width": 1,
                })
            })
            .collect()
    };
    let signals = signal_entries(true);
    let optional_signals = signal_entries(false);
    let parameters: Vec<Value> = m
        .options
        .iter()
        .map(|o| {
            use wavecrux_sigrok_bridge_ipc::OptionKind;
            // Translate IPC OptionKind to the WaveCrux manifest "kind" strings
            // defined in wavecrux_decoder.h. WaveCrux has no float type yet;
            // float options fall back to "string" so users can type a value.
            let kind = match o.kind {
                OptionKind::Bool => "boolean",
                OptionKind::Int => "integer",
                OptionKind::Float => "string",
                OptionKind::Enum => "enumeration",
                OptionKind::String => "string",
            };
            // WaveCrux fills an enumeration's picker from `enum_values`.
            let enum_values: Vec<String> = o
                .choices
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                })
                .collect();
            json!({
                "name": o.name,
                "description": o.description,
                "kind": kind,
                "default": o.default,
                "choices": o.choices,
                "enum_values": enum_values,
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
        "optional_signals": optional_signals,
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
        assert_eq!(params[0]["kind"], "integer");
    }

    #[test]
    fn manifest_uses_wavecrux_keys_for_optional_channels_and_enum_values() {
        let m = DecoderManifest {
            id: "sigrok.pwm".into(),
            display_name: "PWM".into(),
            description: "Pulse-width modulation analyzer.".into(),
            channels: vec![
                DecoderChannel {
                    name: "data".into(),
                    description: "Signal".into(),
                    required: true,
                },
                DecoderChannel {
                    name: "clk".into(),
                    description: "Clock".into(),
                    required: false,
                },
            ],
            options: vec![DecoderOption {
                name: "polarity".into(),
                description: "Active level".into(),
                kind: OptionKind::Enum,
                default: serde_json::json!("active-high"),
                choices: vec![
                    serde_json::json!("active-high"),
                    serde_json::json!("active-low"),
                ],
            }],
            annotations: vec![],
            tags: vec![],
        };
        let v: serde_json::Value = serde_json::from_str(&build_manifest_json(&m)).unwrap();
        let signals = v["signals"].as_array().unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0]["name"], "data");
        let optional = v["optional_signals"].as_array().unwrap();
        assert_eq!(optional.len(), 1);
        assert_eq!(optional[0]["name"], "clk");
        assert_eq!(
            v["parameters"][0]["enum_values"],
            serde_json::json!(["active-high", "active-low"])
        );
    }
}
