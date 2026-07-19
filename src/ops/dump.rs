//! The hidden `__dump-commands` subcommand — clap introspection to JSON
//! (`libviprs-org/cli/SCHEMA_V2.md` §3).
//!
//! Walks the assembled CLI (the derived pyramid/info/plan/test-image commands
//! plus every family's [`commands()`](super::commands), `CLI_CONTRACT.md` §6)
//! and prints the SCHEMA_V2 §3.1 document. It performs **no** image work,
//! touches no PDF paths (pdfium-free), and is safe to run in the docs build.

use clap::{ArgAction, Command};
use serde_json::{Value, json};

use super::{CommandMeta, FAMILIES};

/// Build the hidden `__dump-commands` clap command.
pub fn command() -> Command {
    Command::new("__dump-commands")
        .hide(true)
        .about("Dump the assembled command registry as JSON (SCHEMA_V2 §3.1).")
        .arg(
            clap::Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .help("Emit the registry as a JSON document on stdout"),
        )
}

/// Serialize the assembled CLI into the SCHEMA_V2 §3.1 JSON document.
///
/// Commands are emitted sorted by name; the hidden `__dump-commands` command is
/// itself excluded. Family/shape/oracle-class come from each family's
/// [`metas()`](super::commands); the built-in pyramid/info/plan/test-image
/// commands carry `family: "builtin"` with `null` shape / oracle class (they
/// are not ops).
pub fn dump_commands_json(cli: &Command) -> Value {
    // name -> (family, shape, oracle_class), gathered from the family registry.
    let mut meta: std::collections::BTreeMap<String, (&'static str, CommandMeta)> =
        std::collections::BTreeMap::new();
    for fam in FAMILIES {
        for cm in (fam.metas)() {
            meta.insert(cm.name.to_string(), (fam.name, cm));
        }
    }

    let mut commands: Vec<Value> = Vec::new();
    for sub in cli.get_subcommands() {
        if sub.is_hide_set() {
            continue; // skip __dump-commands and any other hidden command
        }
        let name = sub.get_name();
        let about = sub.get_about().map(|s| s.to_string());
        let (family, shape, oracle_class): (&str, Value, Value) = match meta.get(name) {
            Some((fam, cm)) => (
                fam,
                json!(cm.shape.as_str()),
                json!(cm.oracle_class.as_str()),
            ),
            None => ("builtin", Value::Null, Value::Null),
        };

        commands.push(json!({
            "name": name,
            "about": about,
            "family": family,
            "shape": shape,
            "oracle_class": oracle_class,
            "positionals": positionals(sub),
            "flags": flags(sub),
        }));
    }

    commands.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

    json!({
        "viprs_version": env!("CARGO_PKG_VERSION"),
        "generated_by": "viprs __dump-commands",
        "commands": commands,
    })
}

/// Positional args in declaration order (= vips positional order).
///
/// `Arg::get_index` is only populated once clap *builds* the command; the
/// assembled tree here is unbuilt, so the 1-based `index` is derived from the
/// declaration order that `get_positionals` preserves.
///
/// vips op-selectors are POSITIONALS (`morph`'s `erode|dilate`, `countlines`'
/// `horizontal|vertical`, and later `round`/`boolean`/`relational`/…), so the
/// `value_name` and `possible_values` (enum choices) are emitted here exactly
/// as they are for flags — otherwise the enum choices vanish from the docs
/// (`libviprs-org/cli/SCHEMA_V2.md` §3.1).
fn positionals(cmd: &Command) -> Vec<Value> {
    cmd.get_positionals()
        .enumerate()
        .map(|(i, a)| {
            let id = a.get_id().as_str();
            let value_name = a
                .get_value_names()
                .and_then(|names| names.first())
                .map(|s| s.to_string())
                .unwrap_or_else(|| id.to_ascii_uppercase());

            let possible: Vec<String> = a
                .get_possible_values()
                .iter()
                .map(|p| p.get_name().to_string())
                .collect();
            let possible_values = if possible.is_empty() {
                Value::Null
            } else {
                json!(possible)
            };

            json!({
                "name": id,
                "index": i + 1,
                "required": a.is_required_set(),
                "value_name": value_name,
                "possible_values": possible_values,
                "help": a.get_help().map(|s| s.to_string()),
            })
        })
        .collect()
}

/// Every non-positional arg, minus the auto-generated help/version.
fn flags(cmd: &Command) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for a in cmd.get_arguments() {
        if a.is_positional() {
            continue;
        }
        let id = a.get_id().as_str();
        if id == "help" || id == "version" {
            continue;
        }
        let action = a.get_action();
        let takes_value = !matches!(
            action,
            ArgAction::SetTrue
                | ArgAction::SetFalse
                | ArgAction::Count
                | ArgAction::Help
                | ArgAction::HelpShort
                | ArgAction::HelpLong
                | ArgAction::Version
        );
        let multiple = matches!(action, ArgAction::Append | ArgAction::Count);

        let value_name = a
            .get_value_names()
            .and_then(|names| names.first())
            .map(|s| s.to_string())
            .unwrap_or_else(|| a.get_long().unwrap_or(id).to_ascii_uppercase());

        let default = a
            .get_default_values()
            .first()
            .map(|s| s.to_string_lossy().into_owned());

        let possible: Vec<String> = a
            .get_possible_values()
            .iter()
            .map(|p| p.get_name().to_string())
            .collect();
        let possible_values = if possible.is_empty() {
            Value::Null
        } else {
            json!(possible)
        };

        out.push(json!({
            "long": a.get_long(),
            "short": a.get_short().map(|c| c.to_string()),
            "value_name": value_name,
            "takes_value": takes_value,
            "multiple": multiple,
            "required": a.is_required_set(),
            "default": default,
            "possible_values": possible_values,
            "help": a.get_help().map(|s| s.to_string()),
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump() -> Value {
        dump_commands_json(&super::super::assembled_cli())
    }

    #[test]
    fn top_level_shape_is_schema_v2() {
        let d = dump();
        assert_eq!(d["generated_by"], "viprs __dump-commands");
        assert!(d["viprs_version"].is_string());
        assert!(d["commands"].is_array());
    }

    #[test]
    fn dump_excludes_itself_and_sorts_by_name() {
        let d = dump();
        let names: Vec<&str> = d["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert!(
            !names.contains(&"__dump-commands"),
            "hidden command must be excluded"
        );
        assert!(names.contains(&"morph"));
        assert!(names.contains(&"pyramid"));
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "commands must be sorted by name");
    }

    #[test]
    fn morph_command_carries_family_shape_oracle_and_positionals() {
        let d = dump();
        let morph = d["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "morph")
            .unwrap();
        assert_eq!(morph["family"], "morphology");
        assert_eq!(morph["shape"], "image->image");
        assert_eq!(morph["oracle_class"], "EXACT");
        let pos = morph["positionals"].as_array().unwrap();
        assert_eq!(pos[0]["name"], "IN");
        assert_eq!(pos[0]["index"], 1);
        // The op-selector positional (erode|dilate) must carry its enum
        // choices and value_name, mirroring flags (A4 / SCHEMA_V2 §3.1).
        let op = pos.iter().find(|p| p["name"] == "OP").unwrap();
        assert_eq!(op["value_name"], "erode|dilate");
        assert_eq!(
            op["possible_values"],
            serde_json::json!(["erode", "dilate"])
        );
        // A plain positional without choices reports null possible_values.
        assert!(pos[0]["possible_values"].is_null());
        assert_eq!(pos[0]["value_name"], "IN");
        // The decode-limit flags appear like any other flag.
        let flags = morph["flags"].as_array().unwrap();
        assert!(flags.iter().any(|f| f["long"] == "max-width"));
        assert!(flags.iter().any(|f| f["long"] == "max-alloc-bytes"));
    }

    #[test]
    fn builtin_pyramid_has_null_shape() {
        let d = dump();
        let pyramid = d["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "pyramid")
            .unwrap();
        assert_eq!(pyramid["family"], "builtin");
        assert!(pyramid["shape"].is_null());
        assert!(pyramid["oracle_class"].is_null());
    }
}
