//! The three glyph sets (DESIGN §3): `ascii`, `unicode` and `nerd`, and
//! the `icons` setting that picks one (`auto` by default: `nerd` when a
//! Nerd Font is installed, else `ascii`). Every state glyph is one cell
//! wide; borders and the mascot do not belong to the sets (they follow the
//! locale instead).
//!
//! Detection only lists font folders; it never opens a font file or runs
//! a command.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The `icons` setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum IconChoice {
    /// `nerd` when a Nerd Font is installed, else `ascii`.
    #[default]
    Auto,
    /// Always [`IconSet::Ascii`].
    Ascii,
    /// Always [`IconSet::Unicode`].
    Unicode,
    /// Always [`IconSet::Nerd`].
    Nerd,
}

impl IconChoice {
    /// Parses a `--icons` value.
    #[must_use]
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "auto" => Some(Self::Auto),
            "ascii" => Some(Self::Ascii),
            "unicode" => Some(Self::Unicode),
            "nerd" => Some(Self::Nerd),
            _ => None,
        }
    }

    /// Returns the glyph set this choice means.
    ///
    /// # Arguments
    ///
    /// * `nerd_font` - Whether a Nerd Font is usable; asked only for
    ///   [`IconChoice::Auto`].
    #[must_use]
    pub(crate) fn resolve(self, nerd_font: impl FnOnce() -> bool) -> IconSet {
        match self {
            Self::Auto if nerd_font() => IconSet::Nerd,
            Self::Auto | Self::Ascii => IconSet::Ascii,
            Self::Unicode => IconSet::Unicode,
            Self::Nerd => IconSet::Nerd,
        }
    }
}

/// How many folder levels below a font folder are searched; Linux distros
/// nest fonts as `fonts/truetype/<family>/`.
const FONT_DEPTH: u8 = 3;

/// Returns the folders where the user's and the system's fonts live on
/// macOS and Linux.
///
/// # Arguments
///
/// * `home` - The user's home directory, if known.
#[must_use]
pub(crate) fn font_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let user = ["Library/Fonts", ".local/share/fonts", ".fonts"];
    let system = [
        "/Library/Fonts",
        "/usr/local/share/fonts",
        "/usr/share/fonts",
    ];
    home.into_iter()
        .flat_map(|h| user.map(|d| h.join(d)))
        .chain(system.map(PathBuf::from))
        .collect()
}

/// Returns whether any of `dirs` holds a file or folder with "nerd" in its
/// name, which every Nerd Font release has (`JetBrainsMonoNerdFont-…`).
///
/// An installed font is the best signal there is: the terminal's own font
/// cannot be queried (ARCHITECTURE §12), so a user whose terminal uses
/// another font sets `icons` by hand. Unreadable folders count as empty.
#[must_use]
pub(crate) fn nerd_font_installed(dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|dir| has_nerd_name(dir, FONT_DEPTH))
}

/// Searches `dir` and `depth` levels below it for a name containing
/// "nerd"; symlinked folders are not followed.
fn has_nerd_name(dir: &Path, depth: u8) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("nerd")
            || (depth > 0
                && entry.file_type().is_ok_and(|t| t.is_dir())
                && has_nerd_name(&entry.path(), depth - 1))
    })
}

/// A set of state glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IconSet {
    /// Plain ASCII: unambiguous in every terminal and width setting.
    Ascii,
    /// Narrow Unicode symbols.
    Unicode,
    /// Nerd Font private-use glyphs.
    Nerd,
}

/// A glyph with a meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    /// Your turn.
    YourTurn,
    /// Needs you.
    NeedsYou,
    /// Failed.
    Failed,
    /// Wrapped.
    Wrapped,
    /// Stopped.
    Stopped,
    /// A subagent row.
    Subagent,
    /// The focus marker on the selected row.
    Marker,
}

impl IconSet {
    /// Returns the glyph for `icon` in this set (DESIGN §3 table).
    #[must_use]
    pub(crate) const fn icon(self, icon: Icon) -> char {
        match (self, icon) {
            (Self::Ascii, Icon::YourTurn) => '~',
            (Self::Unicode, Icon::YourTurn) => '»',
            (Self::Nerd, Icon::YourTurn) => '\u{f0e7}',
            (Self::Ascii | Self::Unicode, Icon::NeedsYou) => '!',
            (Self::Nerd, Icon::NeedsYou) => '\u{f071}',
            (Self::Ascii, Icon::Failed) => 'x',
            (Self::Unicode, Icon::Failed) => '✗',
            (Self::Nerd, Icon::Failed) => '\u{f00d}',
            (Self::Ascii, Icon::Wrapped) => '+',
            (Self::Unicode, Icon::Wrapped) => '✓',
            (Self::Nerd, Icon::Wrapped) => '\u{f00c}',
            (Self::Ascii, Icon::Stopped) => '#',
            (Self::Unicode, Icon::Stopped) => '▪',
            (Self::Nerd, Icon::Stopped) => '\u{f04d}',
            (Self::Ascii, Icon::Subagent) => '*',
            (Self::Unicode, Icon::Subagent) => '◦',
            (Self::Nerd, Icon::Subagent | Icon::Marker) => '\u{f0da}',
            (Self::Ascii, Icon::Marker) => '>',
            (Self::Unicode, Icon::Marker) => '▸',
        }
    }

    /// Returns the spinner frames (DESIGN §3 "running").
    #[must_use]
    pub(crate) const fn spinner(self) -> &'static [char] {
        match self {
            Self::Ascii => &['|', '/', '-', '\\'],
            Self::Unicode | Self::Nerd => &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'],
        }
    }

    /// Returns how MCP server `name` shows on a card (DESIGN §5.2): in the
    /// `nerd` set a logo alone when a word of the name is in
    /// [`MCP_BRANDS`], else the glyph of its kind ([`MCP_KINDS`]) or the
    /// plug, and the name; the name in the other sets.
    #[must_use]
    pub(crate) fn mcp(self, name: &str) -> String {
        match self {
            Self::Ascii | Self::Unicode => name.to_owned(),
            Self::Nerd => {
                let lower = name.to_ascii_lowercase();
                let words: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).collect();
                let find = |table: &[(&[&str], char)]| {
                    let row = table
                        .iter()
                        .find(|(known, _)| known.iter().any(|w| words.contains(w)))?;
                    Some(row.1)
                };
                match find(MCP_BRANDS) {
                    Some(logo) => logo.to_string(),
                    None => format!("{} {name}", find(MCP_KINDS).unwrap_or(MCP_PLUG)),
                }
            }
        }
    }
}

/// The `nerd` glyph of an MCP server without a brand glyph (plug).
const MCP_PLUG: char = '\u{f1e6}';

/// Words of an MCP server's name and the `nerd` logo they stand for, shown
/// without the name (DESIGN §3); the first matching row wins. Code points
/// are from Nerd Fonts' `glyphnames.json`; the `nf-` name is on each row.
const MCP_BRANDS: &[(&[&str], char)] = &[
    (&["github"], '\u{f09b}'),                   // nf-fa-github
    (&["gitlab"], '\u{f296}'),                   // nf-fa-gitlab
    (&["bitbucket"], '\u{f171}'),                // nf-fa-bitbucket
    (&["slack"], '\u{f198}'),                    // nf-fa-slack
    (&["discord"], '\u{f066f}'),                 // nf-md-discord
    (&["telegram"], '\u{f2c6}'),                 // nf-fa-telegram
    (&["figma"], '\u{ef47}'),                    // nf-fa-figma
    (&["blender"], '\u{f00ab}'),                 // nf-md-blender_software
    (&["sketch"], '\u{e8a3}'),                   // nf-dev-sketch
    (&["photoshop"], '\u{e7b8}'),                // nf-dev-photoshop
    (&["illustrator"], '\u{e7b4}'),              // nf-dev-illustrator
    (&["canva"], '\u{e77c}'),                    // nf-dev-canva
    (&["xd"], '\u{e8e9}'),                       // nf-dev-xd
    (&["unity"], '\u{f06af}'),                   // nf-md-unity
    (&["unreal"], '\u{f09b1}'),                  // nf-md-unreal
    (&["godot"], '\u{e7ee}'),                    // nf-dev-godot
    (&["chrome", "chromium"], '\u{f268}'),       // nf-fa-chrome
    (&["firefox"], '\u{f269}'),                  // nf-fa-firefox
    (&["safari"], '\u{f267}'),                   // nf-fa-safari
    (&["playwright"], '\u{e863}'),               // nf-dev-playwright
    (&["puppeteer"], '\u{e874}'),                // nf-dev-puppeteer
    (&["selenium"], '\u{e89d}'),                 // nf-dev-selenium
    (&["gmail"], '\u{f02ab}'),                   // nf-md-gmail
    (&["drive", "gdrive"], '\u{f02b6}'),         // nf-md-google_drive
    (&["maps"], '\u{f05f5}'),                    // nf-md-google_maps
    (&["firebase"], '\u{f0967}'),                // nf-md-firebase
    (&["google", "gcp"], '\u{f1a0}'),            // nf-fa-google
    (&["aws"], '\u{f0e0f}'),                     // nf-md-aws
    (&["amazon"], '\u{f270}'),                   // nf-fa-amazon
    (&["azure"], '\u{f0805}'),                   // nf-md-microsoft_azure
    (&["teams"], '\u{f02bb}'),                   // nf-md-microsoft_teams
    (&["outlook"], '\u{f0d22}'),                 // nf-md-microsoft_outlook
    (&["excel"], '\u{f138f}'),                   // nf-md-microsoft_excel
    (&["onedrive"], '\u{f03ca}'),                // nf-md-microsoft_onedrive
    (&["sharepoint"], '\u{f1391}'),              // nf-md-microsoft_sharepoint
    (&["microsoft"], '\u{f0372}'),               // nf-md-microsoft
    (&["cloudflare"], '\u{e792}'),               // nf-dev-cloudflare
    (&["vercel"], '\u{e8d3}'),                   // nf-dev-vercel
    (&["netlify"], '\u{e83c}'),                  // nf-dev-netlify
    (&["heroku"], '\u{e77b}'),                   // nf-dev-heroku
    (&["digitalocean"], '\u{e7ae}'),             // nf-dev-digitalocean
    (&["supabase"], '\u{e8b6}'),                 // nf-dev-supabase
    (&["postgres", "postgresql"], '\u{e76e}'),   // nf-dev-postgresql
    (&["sqlite"], '\u{e7c4}'),                   // nf-dev-sqlite
    (&["mysql"], '\u{e704}'),                    // nf-dev-mysql
    (&["mariadb"], '\u{e828}'),                  // nf-dev-mariadb
    (&["mongodb", "mongo"], '\u{e7a4}'),         // nf-dev-mongodb
    (&["redis"], '\u{e76d}'),                    // nf-dev-redis
    (&["elasticsearch", "elastic"], '\u{e7ca}'), // nf-dev-elasticsearch
    (&["kafka"], '\u{f100f}'),                   // nf-md-apache_kafka
    (&["docker"], '\u{f0868}'),                  // nf-md-docker
    (&["kubernetes", "k8s"], '\u{f10fe}'),       // nf-md-kubernetes
    (&["terraform"], '\u{f1062}'),               // nf-md-terraform
    (&["ansible"], '\u{f109a}'),                 // nf-md-ansible
    (&["jenkins"], '\u{e767}'),                  // nf-dev-jenkins
    (&["nginx"], '\u{e776}'),                    // nf-dev-nginx
    (&["notion"], '\u{e848}'),                   // nf-dev-notion
    (&["jira"], '\u{f0303}'),                    // nf-md-jira
    (&["confluence"], '\u{e799}'),               // nf-dev-confluence
    (&["atlassian"], '\u{f0804}'),               // nf-md-atlassian
    (&["trello"], '\u{f0532}'),                  // nf-md-trello
    (&["evernote"], '\u{f0204}'),                // nf-md-evernote
    (&["obsidian"], '\u{e6bb}'),                 // nf-custom-obsidian
    (&["sentry"], '\u{e89f}'),                   // nf-dev-sentry
    (&["grafana"], '\u{e7f3}'),                  // nf-dev-grafana
    (&["prometheus"], '\u{e870}'),               // nf-dev-prometheus
    (&["datadog"], '\u{e902}'),                  // nf-dev-datadog
    (&["stripe"], '\u{f1f5}'),                   // nf-fa-cc_stripe
    (&["paypal"], '\u{f1ed}'),                   // nf-fa-paypal
    (&["salesforce"], '\u{f088e}'),              // nf-md-salesforce
    (&["hubspot"], '\u{f0d17}'),                 // nf-md-hubspot
    (&["twilio"], '\u{e94e}'),                   // nf-dev-twilio
    (&["mailchimp"], '\u{ee67}'),                // nf-fa-mailchimp
    (&["algolia"], '\u{e70a}'),                  // nf-dev-algolia
    (&["sanity"], '\u{e899}'),                   // nf-dev-sanity
    (&["wordpress"], '\u{f05b4}'),               // nf-md-wordpress
    (&["laravel"], '\u{f0ad0}'),                 // nf-md-laravel
    (&["react"], '\u{f0708}'),                   // nf-md-react
    (&["vue"], '\u{e6a0}'),                      // nf-seti-vue
    (&["angular"], '\u{f06b2}'),                 // nf-md-angular
    (&["svelte"], '\u{e8b7}'),                   // nf-dev-svelte
    (&["nextjs"], '\u{e83e}'),                   // nf-dev-nextjs
    (&["nuxt"], '\u{f1106}'),                    // nf-md-nuxt
    (&["astro"], '\u{e735}'),                    // nf-dev-astro
    (&["tailwind", "tailwindcss"], '\u{f13ff}'), // nf-md-tailwind
    (&["graphql"], '\u{f0877}'),                 // nf-md-graphql
    (&["postman"], '\u{e86b}'),                  // nf-dev-postman
    (&["swagger", "openapi"], '\u{e8b8}'),       // nf-dev-swagger
    (&["storybook"], '\u{e8b3}'),                // nf-dev-storybook
    (&["jest"], '\u{e807}'),                     // nf-dev-jest
    (&["npm"], '\u{f06f7}'),                     // nf-md-npm
    (&["node", "nodejs"], '\u{f0399}'),          // nf-md-nodejs
    (&["bun"], '\u{e76f}'),                      // nf-dev-bun
    (&["python"], '\u{f0320}'),                  // nf-md-language_python
    (&["rust"], '\u{f1617}'),                    // nf-md-language_rust
    (&["typescript"], '\u{f06e6}'),              // nf-md-language_typescript
    (&["javascript"], '\u{f031e}'),              // nf-md-language_javascript
    (&["php"], '\u{f031f}'),                     // nf-md-language_php
    (&["ruby"], '\u{e739}'),                     // nf-dev-ruby
    (&["java"], '\u{f0b37}'),                    // nf-md-language_java
    (&["swift"], '\u{f06e5}'),                   // nf-md-language_swift
    (&["kotlin"], '\u{f1219}'),                  // nf-md-language_kotlin
    (&["dart"], '\u{e798}'),                     // nf-dev-dart
    (&["flutter"], '\u{e7dd}'),                  // nf-dev-flutter
    (&["vscode"], '\u{ec29}'),                   // nf-cod-vscode
    (&["xcode"], '\u{e8e8}'),                    // nf-dev-xcode
    (&["openai", "chatgpt"], '\u{ec81}'),        // nf-cod-openai
    (&["claude", "anthropic"], '\u{ec82}'),      // nf-cod-claude
    (&["youtube"], '\u{f05c3}'),                 // nf-md-youtube
    (&["spotify"], '\u{f04c7}'),                 // nf-md-spotify
    (&["dropbox"], '\u{f01e3}'),                 // nf-md-dropbox
    (&["twitter"], '\u{f0544}'),                 // nf-md-twitter
    (&["reddit"], '\u{f044d}'),                  // nf-md-reddit
    (&["twitch"], '\u{f0543}'),                  // nf-md-twitch
    (&["stackoverflow"], '\u{e710}'),            // nf-dev-stackoverflow
    (&["wikipedia"], '\u{f05ac}'),               // nf-md-wikipedia
    (&["apple", "macos", "ios"], '\u{f0035}'),   // nf-md-apple
    (&["linux"], '\u{f033d}'),                   // nf-md-linux
    (&["windows"], '\u{f05b3}'),                 // nf-md-microsoft_windows
    (&["android"], '\u{f0032}'),                 // nf-md-android
];

/// Words of an MCP server's name without a logo and the `nerd` glyph for
/// what it is, shown before the name; the first matching row wins.
const MCP_KINDS: &[(&[&str], char)] = &[
    (
        &[
            "payload",
            "payloadcms",
            "strapi",
            "contentful",
            "microcms",
            "cms",
        ],
        '\u{f059f}',
    ), // nf-md-web
    (&["filesystem", "files", "file", "fs"], '\u{f07b}'), // nf-fa-folder
    (&["memory"], '\u{f035b}'),                           // nf-md-memory
    (&["thinking", "sequential"], '\u{f09d1}'),           // nf-md-brain
    (&["db", "database", "sql"], '\u{f1c0}'),             // nf-fa-database
    (&["fetch", "web", "http", "browser", "devtools"], '\u{f0ac}'), // nf-fa-globe
    (&["search"], '\u{f002}'),                            // nf-fa-search
    (&["terminal", "shell", "bash", "repl"], '\u{f120}'), // nf-fa-terminal
    (&["time", "clock"], '\u{f017}'),                     // nf-fa-clock
    (&["calendar"], '\u{f073}'),                          // nf-fa-calendar
    (&["mail", "email"], '\u{f01ee}'),                    // nf-md-email
    (&["docs", "context7", "documentation"], '\u{f02d}'), // nf-fa-book
    (&["image", "images", "screenshot"], '\u{f03e}'),     // nf-fa-image
    (&["video"], '\u{f03d}'),                             // nf-fa-video
    (&["music", "audio"], '\u{f001}'),                    // nf-fa-music
    (&["map"], '\u{f279}'),                               // nf-fa-map
    (&["api"], '\u{f109b}'),                              // nf-md-api
    (&["code"], '\u{f121}'),                              // nf-fa-code
    (&["test", "tests"], '\u{f0668}'),                    // nf-md-test_tube
    (&["debug"], '\u{f188}'),                             // nf-fa-bug
    (&["agent", "ai", "llm"], '\u{f06a9}'),               // nf-md-robot
    (&["tools"], '\u{f1064}'),                            // nf-md-tools
    (&["cloud"], '\u{f0c2}'),                             // nf-fa-cloud
    (&["server"], '\u{f233}'),                            // nf-fa-server
    (&["calculator", "math"], '\u{f1ec}'),                // nf-fa-calculator
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_servers_show_a_brand_glyph_only_in_the_nerd_set() {
        let cases = [
            (IconSet::Nerd, "plugin:slack:slack", "\u{f198}"),
            (IconSet::Nerd, "GitHub", "\u{f09b}"),
            (IconSet::Nerd, "claude.ai Google Drive", "\u{f02b6}"),
            (IconSet::Nerd, "Figma", "\u{ef47}"),
            (IconSet::Nerd, "blender", "\u{f00ab}"),
            (IconSet::Nerd, "chrome-devtools", "\u{f268}"),
            (IconSet::Nerd, "payloadcms", "\u{f059f} payloadcms"),
            (IconSet::Nerd, "miko-manager", "\u{f1e6} miko-manager"),
            (IconSet::Nerd, "webdriver", "\u{f1e6} webdriver"),
            (IconSet::Ascii, "github", "github"),
            (IconSet::Unicode, "blender", "blender"),
        ];
        for (set, name, want) in cases {
            assert_eq!(set.mcp(name), want, "{set:?} {name}");
        }
    }

    #[test]
    fn mcp_glyphs_are_one_cell_and_each_word_stands_for_one_glyph() {
        let mut words = Vec::new();
        for (known, glyph) in MCP_BRANDS.iter().chain(MCP_KINDS) {
            let width = ratatui::text::Span::raw(glyph.to_string()).width();
            assert_eq!(width, 1, "{known:?}");
            words.extend_from_slice(known);
        }
        let count = words.len();
        words.sort_unstable();
        words.dedup();
        assert_eq!(words.len(), count, "a word is in two rows");
    }

    const ALL: [Icon; 7] = [
        Icon::YourTurn,
        Icon::NeedsYou,
        Icon::Failed,
        Icon::Wrapped,
        Icon::Stopped,
        Icon::Subagent,
        Icon::Marker,
    ];

    #[test]
    fn ascii_and_unicode_glyphs_are_one_cell_and_narrow() {
        for set in [IconSet::Ascii, IconSet::Unicode] {
            let glyphs = ALL
                .iter()
                .map(|i| set.icon(*i))
                .chain(set.spinner().iter().copied());
            for g in glyphs {
                let width = ratatui::text::Span::raw(g.to_string()).width();
                assert_eq!(width, 1, "{set:?} {g:?}");
            }
        }
    }

    #[test]
    fn auto_picks_nerd_only_when_a_nerd_font_is_installed() {
        let root = std::env::temp_dir().join(format!("mc-fonts-{}", std::process::id()));
        let nested = root.join("truetype/jetbrains");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("DejaVuSansMono.ttf"), "").unwrap();
        let dirs = [root.join("missing"), root.clone()];
        assert!(!nerd_font_installed(&dirs));
        std::fs::write(nested.join("JetBrainsMonoNerdFont-Regular.ttf"), "").unwrap();
        assert!(nerd_font_installed(&dirs));
        std::fs::remove_dir_all(&root).unwrap();

        let cases = [
            (IconChoice::Auto, true, IconSet::Nerd),
            (IconChoice::Auto, false, IconSet::Ascii),
            (IconChoice::Ascii, true, IconSet::Ascii),
            (IconChoice::Unicode, true, IconSet::Unicode),
            (IconChoice::Nerd, false, IconSet::Nerd),
        ];
        for (choice, font, want) in cases {
            assert_eq!(choice.resolve(|| font), want, "{choice:?} {font}");
        }
    }

    #[test]
    fn states_are_distinct_within_a_set() {
        for set in [IconSet::Ascii, IconSet::Unicode, IconSet::Nerd] {
            let states: Vec<char> = ALL[..5].iter().map(|i| set.icon(*i)).collect();
            let mut unique = states.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), states.len(), "{set:?}");
        }
    }
}
