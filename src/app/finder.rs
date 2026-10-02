//! The finder (`fp`, `ff`, `fg`): a popup that searches the open workspace
//! while the user types, as telescope.nvim does.
//!
//! `fp` matches project names, `ff` file names and `fg` file contents. The
//! file names and the matching lines come from ripgrep, which the event
//! loop runs off the UI thread ([`Cmd::Find`]); this module ranks, picks
//! and never reads a file itself.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::model::{Cmd, Focus, Model, Overlay};

/// Most rows the finder keeps to show, and most lines taken from one grep.
pub(crate) const ROWS_MAX: usize = 500;

/// Most file names taken from one workspace; the ones past it are not found.
pub(crate) const FILES_MAX: usize = 200_000;

/// What the finder searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// The workspace's project names (`fp`).
    Projects,
    /// The workspace's file names (`ff`).
    Files,
    /// The workspace's file contents, by regex (`fg`).
    Grep,
}

impl Source {
    /// Returns the title of the finder's popup.
    #[must_use]
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Projects => "find project",
            Self::Files => "find file",
            Self::Grep => "grep",
        }
    }
}

/// The finder's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finder {
    /// What it searches.
    pub source: Source,
    /// What the user typed.
    pub query: String,
    /// Highlighted row of [`Finder::rows`].
    pub selected: usize,
    /// Everything `fp` and `ff` match against: the project names, or the
    /// file paths relative to the workspace.
    items: Vec<String>,
    /// The rows shown, best first: matching items, or for `fg` ripgrep's
    /// `path:line:text` lines.
    pub rows: Vec<String>,
    /// How many items match; [`Finder::rows`] holds the first [`ROWS_MAX`].
    pub total: usize,
    /// Whether ripgrep's answer is still awaited.
    pub searching: bool,
}

impl Finder {
    /// Fills [`Finder::rows`] with the items matching the query, best
    /// first (see [`tier`]; within a tier the shorter item), in list order
    /// while nothing is typed.
    fn rank(&mut self) {
        let needle = self.query.as_bytes();
        let mut hits: Vec<(u8, usize, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| Some((tier(item, needle)?, item.len(), i)))
            .collect();
        if !needle.is_empty() {
            hits.sort_unstable();
        }
        self.total = hits.len();
        self.rows = hits
            .iter()
            .take(ROWS_MAX)
            .map(|&(_, _, i)| self.items[i].clone())
            .collect();
        self.selected = 0;
    }
}

/// Returns how well `item` matches `needle`, ASCII case ignored and lower
/// is better: 0 when the file name contains it, 1 when the path does, 2
/// when its characters appear in the path in order (the fuzzy match), and
/// `None` when it does not match.
fn tier(item: &str, needle: &[u8]) -> Option<u8> {
    let contains = |hay: &[u8]| {
        needle.is_empty()
            || hay
                .windows(needle.len())
                .any(|w| w.eq_ignore_ascii_case(needle))
    };
    let name = item.rsplit('/').next().unwrap_or(item);
    if contains(name.as_bytes()) {
        return Some(0);
    }
    if contains(item.as_bytes()) {
        return Some(1);
    }
    let mut rest = item.bytes();
    needle
        .iter()
        .all(|n| rest.any(|h| h.eq_ignore_ascii_case(n)))
        .then_some(2)
}

impl Model {
    /// Opens the finder on `source`.
    ///
    /// # Returns
    ///
    /// For `ff`, the command that lists the workspace's files.
    pub(super) fn open_finder(&mut self, source: Source) -> Option<Cmd> {
        let root = self.root()?.to_path_buf();
        let items = match source {
            Source::Projects => self.projects.iter().map(|p| p.name.clone()).collect(),
            Source::Files | Source::Grep => Vec::new(),
        };
        let mut finder = Finder {
            source,
            query: String::new(),
            selected: 0,
            items,
            rows: Vec::new(),
            total: 0,
            searching: source == Source::Files,
        };
        finder.rank();
        self.overlay = Some(Overlay::Finder(finder));
        self.find_seq += 1;
        (source == Source::Files).then_some(Cmd::Find(root, self.find_seq, None))
    }

    /// Takes ripgrep's lines for search `seq` into the open finder: the
    /// file names of `ff`, or the matching lines of `fg`. A search that is
    /// no longer the last one started is dropped.
    pub(super) fn found(&mut self, seq: u64, lines: Vec<String>) {
        if seq != self.find_seq {
            return;
        }
        let Some(Overlay::Finder(finder)) = &mut self.overlay else {
            return;
        };
        finder.searching = false;
        match finder.source {
            Source::Projects => {}
            Source::Files => {
                finder.items = lines;
                finder.rank();
            }
            Source::Grep => {
                finder.total = lines.len();
                finder.rows = lines;
                finder.selected = 0;
            }
        }
    }

    /// Applies a key to the finder: typing edits the query (`ctrl-u`
    /// clears it) and searches again, `↑`/`↓` move, `enter` opens the
    /// highlighted row, `esc` closes.
    ///
    /// # Returns
    ///
    /// The grep for an edited `fg` query, or what `enter` opens.
    pub(super) fn finder_key(&mut self, mut finder: Finder, key: KeyEvent) -> Option<Cmd> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let last = finder.rows.len().saturating_sub(1);
        let edited = match key.code {
            KeyCode::Esc => return None,
            KeyCode::Enter if !finder.rows.is_empty() => return self.pick(&finder),
            KeyCode::Up => {
                finder.selected = finder.selected.saturating_sub(1);
                false
            }
            KeyCode::Down => {
                finder.selected = (finder.selected + 1).min(last);
                false
            }
            KeyCode::Backspace => finder.query.pop().is_some(),
            KeyCode::Char('u') if ctrl => {
                finder.query.clear();
                true
            }
            KeyCode::Char(ch) if !ctrl => {
                finder.query.push(ch);
                true
            }
            _ => false,
        };
        let mut cmd = None;
        if edited {
            match finder.source {
                Source::Projects | Source::Files => finder.rank(),
                Source::Grep => {
                    self.find_seq += 1;
                    finder.searching = !finder.query.is_empty();
                    if finder.searching {
                        let root = self.root().map(std::path::Path::to_path_buf);
                        let pattern = Some(finder.query.clone());
                        cmd = root.map(|root| Cmd::Find(root, self.find_seq, pattern));
                    } else {
                        finder.rows.clear();
                        finder.total = 0;
                        finder.selected = 0;
                    }
                }
            }
        }
        self.overlay = Some(Overlay::Finder(finder));
        cmd
    }

    /// Opens the finder's highlighted row: a project's sessions, or a file
    /// in the editor (for `fg` at the matching line).
    // ponytail: a path with `:` in it splits wrongly for `fg` and opens a
    // wrong name; use ripgrep's `--null` when one turns up.
    fn pick(&mut self, finder: &Finder) -> Option<Cmd> {
        let row = finder.rows.get(finder.selected)?;
        match finder.source {
            Source::Projects => {
                let path = self.projects.iter().find(|p| p.name == *row)?.path.clone();
                self.filter.clear();
                if self.select_project(&path) {
                    self.card = 0;
                    self.focus = Focus::Sessions;
                }
                None
            }
            Source::Files => Some(Cmd::OpenFile(self.root()?.to_path_buf(), row.into(), None)),
            Source::Grep => {
                let mut parts = row.splitn(3, ':');
                let file = parts.next()?.into();
                let line = parts.next().and_then(|n| n.parse().ok());
                Some(Cmd::OpenFile(self.root()?.to_path_buf(), file, line))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppEvent;
    use crate::app::model::tests::{press, sample};

    fn typed(m: &mut Model, text: &str) -> Option<Cmd> {
        text.chars()
            .map(|ch| m.update(press(KeyCode::Char(ch))))
            .last()
            .flatten()
    }

    fn rows(m: &Model) -> Vec<&str> {
        let Some(Overlay::Finder(finder)) = &m.overlay else {
            panic!("the finder is open");
        };
        finder.rows.iter().map(String::as_str).collect()
    }

    #[test]
    fn ranks_file_name_then_path_then_fuzzy_matches() {
        let cases = [
            ("src/app/model.rs", "MODEL", Some(0)),
            ("src/app/model.rs", "app/mo", Some(1)),
            ("src/app/model.rs", "samrs", Some(2)),
            ("src/app/model.rs", "xyz", None),
            ("src/app/model.rs", "", Some(0)),
        ];
        for (item, needle, want) in cases {
            assert_eq!(tier(item, needle.as_bytes()), want, "{item} {needle}");
        }
    }

    #[test]
    fn fp_finds_a_project_and_opens_its_sessions() {
        let mut m = sample(&["kedai-web", "pasar-mobile", "warung-api"]);
        typed(&mut m, "fp");
        assert_eq!(rows(&m).len(), 3);
        typed(&mut m, "wrg");
        assert_eq!(rows(&m), ["warung-api"]);
        assert_eq!(m.update(press(KeyCode::Enter)), None);
        assert_eq!(
            m.selected_project().map(|p| p.name.as_str()),
            Some("warung-api")
        );
        assert!(m.overlay.is_none() && m.focus == Focus::Sessions);
    }

    #[test]
    fn ff_lists_files_through_ripgrep_and_opens_the_best_match() {
        let mut m = sample(&["a"]);
        let root = m.root().unwrap().to_path_buf();
        let seq = m.find_seq + 1;
        assert_eq!(
            typed(&mut m, "ff"),
            Some(Cmd::Find(root.clone(), seq, None))
        );
        let files = ["a/src/main.rs", "a/main.rs", "a/docs/remain.md"];
        m.update(AppEvent::Found(seq - 1, vec!["stale".into()]));
        assert!(rows(&m).is_empty(), "an older search is dropped");
        m.update(AppEvent::Found(seq, files.map(String::from).to_vec()));
        assert_eq!(rows(&m), files);
        typed(&mut m, "main.rs");
        assert_eq!(rows(&m), ["a/main.rs", "a/src/main.rs"]);
        m.update(press(KeyCode::Down));
        assert_eq!(
            m.update(press(KeyCode::Enter)),
            Some(Cmd::OpenFile(root, "a/src/main.rs".into(), None))
        );
        assert!(m.overlay.is_none());
    }

    #[test]
    fn fg_greps_on_every_edit_and_opens_the_file_at_the_line() {
        let mut m = sample(&["a"]);
        let root = m.root().unwrap().to_path_buf();
        assert_eq!(typed(&mut m, "fg"), None);
        let grep = typed(&mut m, "fn");
        let seq = m.find_seq;
        assert_eq!(grep, Some(Cmd::Find(root.clone(), seq, Some("fn".into()))));
        m.update(AppEvent::Found(
            seq,
            vec!["a/src/main.rs:12:fn main() {".into()],
        ));
        let screen = crate::ui::tests::render(&mut m, 80, 24);
        assert!(
            screen.contains("> a/src/main.rs:12:fn main() {"),
            "{screen}"
        );
        assert_eq!(
            m.update(press(KeyCode::Enter)),
            Some(Cmd::OpenFile(root, "a/src/main.rs".into(), Some(12)))
        );
        typed(&mut m, "fgx");
        assert_eq!(m.update(press(KeyCode::Backspace)), None, "nothing to grep");
        m.update(AppEvent::Found(m.find_seq - 1, vec!["late".into()]));
        assert!(rows(&m).is_empty());
    }
}
