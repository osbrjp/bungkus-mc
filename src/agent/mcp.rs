//! Which MCP servers a session uses: the ones its tool calls name
//! (ARCHITECTURE §5.5).
//!
//! Only server names leave this module, sanitised and capped.

use crate::ui::sanitise::sanitise;

/// Longest server name shown, in characters.
const NAME_MAX: usize = 24;

/// Most servers kept per session.
pub(crate) const SERVERS_MAX: usize = 12;

/// Returns the MCP server a hook's `tool_name` belongs to
/// (`mcp__<server>__<tool>`), without the `claude_ai_` or `plugin_` prefix
/// Claude gives connector and plugin servers.
#[must_use]
pub(crate) fn used(tool_name: &str) -> Option<String> {
    let (server, _) = tool_name.strip_prefix("mcp__")?.split_once("__")?;
    let server = ["claude_ai_", "plugin_"]
        .iter()
        .find_map(|prefix| server.strip_prefix(prefix))
        .unwrap_or(server);
    clean(server)
}

/// Returns `name` as shown: sanitised and cut to [`NAME_MAX`]; `None` when
/// nothing is left.
fn clean(name: &str) -> Option<String> {
    Some(sanitise(name.trim(), NAME_MAX)).filter(|n| !n.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_server_of_an_mcp_tool_call() {
        let cases = [
            ("mcp__blender__get_scene_info", Some("blender")),
            ("mcp__claude_ai_Figma__get_screenshot", Some("Figma")),
            (
                "mcp__plugin_slack_slack__slack_send_message",
                Some("slack_slack"),
            ),
            ("mcp__cut_short…", None),
            ("Bash", None),
            ("mcp____x", None),
        ];
        for (tool, want) in cases {
            assert_eq!(used(tool).as_deref(), want, "{tool}");
        }
    }
}
