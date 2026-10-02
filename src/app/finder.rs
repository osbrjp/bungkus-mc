//! The finder (`fp`, `ff`, `fg`): a popup that searches the open workspace
//! while the user types, as telescope.nvim does.
//!
//! `fp` matches project names, `ff` file names and `fg` file contents. The
//! file names and the matching lines come from ripgrep, which the event
//! loop runs off the UI thread ([`Request::Find`]); this module ranks, picks
//! and never reads a file itself.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::model::{Cmd, Focus, Model, Overlay};

/// Most rows the finder keeps to show, and most lines taken from one grep.
pub(crate) const ROWS_MAX: usize = 500;

/// Most lines read for the preview pane; the popup is at most 30 rows.
pub(crate) const PREVIEW_ROWS: usize = 40;

/// How many lines the preview shows above the line `fg` matched.
pub(crate) const PREVIEW_CONTEXT: usize = 8;

/// Most file names taken from one workspace; the ones past it are not found.
pub(crate) const FILES_MAX: usize = 200_000;

/// Work the finder hands the event loop ([`Cmd::Finder`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Request {
    /// Run ripgrep in this workspace for search `u64`: list its files
    /// (`None`) or the lines matching this pattern.
    Find(PathBuf, u64, Option<String>),
    /// Read the lines of this file the preview shows: its top, or the ones
    /// around this line.
    Preview(PathBuf, Option<u32>),
}

/// What the event loop answers a [`Request`] with.
#[derive(Debug)]
pub(crate) enum Reply {
    /// The lines ripgrep printed for search `u64`.
    Found(u64, Vec<String>),
    /// The lines read for the preview of this file at this line, and which
    /// of them is that line.
    Preview(PathBuf, Option<u32>, Vec<String>, Option<usize>),
}

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
    /// The lines of the highlighted row's file the preview pane shows
    /// (`ff`, `fg`); empty until they are read.
    pub preview: Vec<String>,
    /// The line of [`Finder::preview`] that `fg` matched.
    pub hit: Option<usize>,
}

impl Finder {
    /// Returns the file of the highlighted row, relative to the workspace,
    /// and for `fg` the matching line's number; `None` for `fp` or with no
    /// row.
    // ponytail: a path with `:` in it splits wrongly for `fg` and gives a
    // wrong name; use ripgrep's `--null` when one turns up.
    fn target(&self) -> Option<(&str, Option<u32>)> {
        let row = self.rows.get(self.selected)?;
        match self.source {
            Source::Projects => None,
            Source::Files => Some((row, None)),
            Source::Grep => {
                let mut parts = row.splitn(3, ':');
                Some((parts.next()?, parts.next().and_then(|n| n.parse().ok())))
            }
        }
    }

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
            preview: Vec::new(),
            hit: None,
        };
        finder.rank();
        self.overlay = Some(Overlay::Finder(finder));
        self.find_seq += 1;
        (source == Source::Files).then_some(Cmd::Finder(Request::Find(root, self.find_seq, None)))
    }

    /// Takes ripgrep's lines for search `seq` into the open finder: the
    /// file names of `ff`, or the matching lines of `fg`. A search that is
    /// no longer the last one started is dropped.
    ///
    /// # Returns
    ///
    /// The read of the first row's preview.
    fn found(&mut self, seq: u64, lines: Vec<String>) -> Option<Cmd> {
        if seq != self.find_seq {
            return None;
        }
        let Some(Overlay::Finder(finder)) = &mut self.overlay else {
            return None;
        };
        finder.searching = false;
        finder.preview.clear();
        finder.hit = None;
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
        self.preview_cmd(None)
    }

    /// Empties the open finder's preview and asks for the highlighted
    /// row's, unless that row's file and line are still `shown`.
    ///
    /// # Returns
    ///
    /// The read of the preview, when a file is highlighted.
    fn preview_cmd(&mut self, shown: Option<&(PathBuf, Option<u32>)>) -> Option<Cmd> {
        let root = self.root()?.to_path_buf();
        let Some(Overlay::Finder(finder)) = &mut self.overlay else {
            return None;
        };
        let target = finder.target().map(|(file, line)| (root.join(file), line));
        if target.as_ref() == shown {
            return None;
        }
        finder.preview.clear();
        finder.hit = None;
        target.map(|(file, line)| Cmd::Finder(Request::Preview(file, line)))
    }

    /// Takes the event loop's answer into the open finder.
    ///
    /// # Returns
    ///
    /// The read of the first row's preview, after a search's answer.
    pub(super) fn finder_reply(&mut self, reply: Reply) -> Option<Cmd> {
        match reply {
            Reply::Found(seq, lines) => self.found(seq, lines),
            Reply::Preview(file, line, lines, hit) => {
                self.previewed(&file, line, lines, hit);
                None
            }
        }
    }

    /// Takes the lines read for the preview of `file` at `line` (`hit` is
    /// the matching one) into the finder, when its highlighted row is still
    /// that one.
    fn previewed(
        &mut self,
        file: &Path,
        line: Option<u32>,
        lines: Vec<String>,
        hit: Option<usize>,
    ) {
        let root = self.root().map(Path::to_path_buf);
        if let Some(Overlay::Finder(finder)) = &mut self.overlay
            && let Some(root) = root
            && finder.target().map(|(f, n)| (root.join(f), n)) == Some((file.to_path_buf(), line))
        {
            finder.preview = lines;
            finder.hit = hit;
        }
    }

    /// Applies a key to the finder: typing edits the query (`ctrl-u`
    /// clears it) and searches again, `↑`/`↓` move, `enter` opens the
    /// highlighted row, `esc` closes.
    ///
    /// # Returns
    ///
    /// The grep for an edited `fg` query, the read of a newly highlighted
    /// row's preview, or what `enter` opens.
    pub(super) fn finder_key(&mut self, mut finder: Finder, key: KeyEvent) -> Option<Cmd> {
        let root = self.root().map(Path::to_path_buf);
        let shown = root
            .zip(finder.target())
            .map(|(root, (file, line))| (root.join(file), line));
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
                        let root = self.root().map(Path::to_path_buf);
                        let pattern = Some(finder.query.clone());
                        cmd = root
                            .map(|root| Cmd::Finder(Request::Find(root, self.find_seq, pattern)));
                    } else {
                        finder.rows.clear();
                        finder.total = 0;
                        finder.selected = 0;
                    }
                }
            }
        }
        self.overlay = Some(Overlay::Finder(finder));
        // A grep's answer brings its own preview; the old rows keep theirs.
        cmd.or_else(|| self.preview_cmd(shown.as_ref()))
    }

    /// Opens the finder's highlighted row: a project's sessions, or a file
    /// in the editor (for `fg` at the matching line).
    fn pick(&mut self, finder: &Finder) -> Option<Cmd> {
        if let Some((file, line)) = finder.target() {
            return Some(Cmd::OpenFile(self.root()?.to_path_buf(), file.into(), line));
        }
        let row = finder.rows.get(finder.selected)?;
        let path = self.projects.iter().find(|p| p.name == *row)?.path.clone();
        self.filter.clear();
        if self.select_project(&path) {
            self.card = 0;
            self.focus = Focus::Sessions;
        }
        None
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

    fn want_find(root: PathBuf, seq: u64, pattern: Option<String>) -> Cmd {
        Cmd::Finder(Request::Find(root, seq, pattern))
    }

    fn want_preview(file: PathBuf, line: Option<u32>) -> Cmd {
        Cmd::Finder(Request::Preview(file, line))
    }

    fn found(seq: u64, lines: Vec<String>) -> AppEvent {
        AppEvent::Finder(Reply::Found(seq, lines))
    }

    fn previewed(file: PathBuf, line: Option<u32>, lines: Vec<String>, hit: usize) -> AppEvent {
        AppEvent::Finder(Reply::Preview(file, line, lines, Some(hit)))
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
            Some(want_find(root.clone(), seq, None))
        );
        let files = ["a/src/main.rs", "a/main.rs", "a/docs/remain.md"];
        m.update(found(seq - 1, vec!["stale".into()]));
        assert!(rows(&m).is_empty(), "an older search is dropped");
        m.update(found(seq, files.map(String::from).to_vec()));
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
    fn the_preview_follows_the_highlighted_row_and_drops_a_late_read() {
        let mut m = sample(&["a"]);
        let root = m.root().unwrap().to_path_buf();
        typed(&mut m, "fgfn");
        let hits = ["a/x.rs:12:fn x() {", "a/y.rs:3:fn y() {"];
        let found = found(m.find_seq, hits.map(String::from).to_vec());
        let x = root.join("a/x.rs");
        assert_eq!(m.update(found), Some(want_preview(x.clone(), Some(12))));
        assert_eq!(
            m.update(press(KeyCode::Down)),
            Some(want_preview(root.join("a/y.rs"), Some(3)))
        );
        assert_eq!(m.update(press(KeyCode::Down)), None, "the same row");
        m.update(previewed(x, Some(12), vec!["late".into()], 0));
        let lines = vec!["use z;".to_owned(), "fn y() {".to_owned()];
        m.update(previewed(root.join("a/y.rs"), Some(3), lines, 1));
        let screen = crate::ui::tests::render(&mut m, 120, 40);
        assert!(
            screen.contains("│ use z;") && !screen.contains("late"),
            "{screen}"
        );
        typed(&mut m, "x");
        let screen = crate::ui::tests::render(&mut m, 120, 40);
        assert!(screen.contains("│ fn y() {"), "kept until the grep answers");
    }

    #[test]
    fn fg_greps_on_every_edit_and_opens_the_file_at_the_line() {
        let mut m = sample(&["a"]);
        let root = m.root().unwrap().to_path_buf();
        assert_eq!(typed(&mut m, "fg"), None);
        let grep = typed(&mut m, "fn");
        let seq = m.find_seq;
        assert_eq!(grep, Some(want_find(root.clone(), seq, Some("fn".into()))));
        m.update(found(seq, vec!["a/src/main.rs:12:fn main() {".into()]));
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
        m.update(found(m.find_seq - 1, vec!["late".into()]));
        assert!(rows(&m).is_empty());
    }
}
