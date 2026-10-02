//! The pull request and issue of a session's branch, and the popup that
//! lists a repository's open ones (issue #188).
//!
//! Read through the user's own `gh` CLI (fixed argv, no shell) in the
//! folder a session works in, on a background thread. This module never
//! handles a token and never writes to GitHub; with `gh` missing or not
//! logged in nothing is linked. Titles go through `sanitise()` and a URL is
//! kept only when it is plain `https://`, since it becomes an argument of
//! the desktop's opener.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use serde::Deserialize;

use crate::app::model::{Cmd, Model, Overlay};
use crate::ui::keymap::Action;
use crate::ui::sanitise::sanitise;

/// Longest title kept, in characters.
const TITLE_MAX: usize = 120;

/// Most pull requests and most issues the popup asks `gh` for.
const LIST_MAX: &str = "30";

/// The fields asked of `gh` for every pull request and issue.
const FIELDS: &str = "number,title,state,url";

/// What a [`Link`] points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkKind {
    /// An issue.
    Issue,
    /// A pull request.
    Pr,
}

impl LinkKind {
    /// Returns the words the hint line uses for it.
    const fn words(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::Pr => "pull request",
        }
    }
}

/// One issue or pull request on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Link {
    /// Issue or pull request.
    pub kind: LinkKind,
    /// Its number.
    pub number: u64,
    /// Its title, sanitised; empty when `gh` gave none.
    pub title: String,
    /// Its state in lower case (`open`, `closed`, `merged`).
    pub state: String,
    /// Its `https://` address.
    pub url: String,
}

/// What `gh --json` prints for a pull request or an issue.
#[derive(Debug, Deserialize)]
struct Raw {
    number: u64,
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    state: String,
    #[serde(default, rename = "closingIssuesReferences")]
    closes: Vec<Raw>,
}

impl Raw {
    /// Returns the link, or `None` when the URL is not a plain `https://`
    /// address (it would reach the desktop's opener as an argument).
    fn link(self, kind: LinkKind) -> Option<Link> {
        let plain = !self
            .url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace());
        (self.url.starts_with("https://") && plain).then(|| Link {
            kind,
            number: self.number,
            title: sanitise(&self.title, TITLE_MAX),
            state: sanitise(&self.state, 12).to_lowercase(),
            url: self.url,
        })
    }
}

/// The pull request and issue of one session folder's branch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Links {
    /// The branch they were read for; another branch means read again.
    pub branch: String,
    /// The pull request GitHub has for the branch.
    pub pr: Option<Link>,
    /// The issue the branch is for.
    pub issue: Option<Link>,
}

impl Links {
    /// Returns the link of `kind`, if there is one.
    const fn get(&self, kind: LinkKind) -> Option<&Link> {
        match kind {
            LinkKind::Issue => self.issue.as_ref(),
            LinkKind::Pr => self.pr.as_ref(),
        }
    }

    /// Returns what the card's git line adds: `#188 PR #189`, either part
    /// alone, or nothing.
    #[must_use]
    pub(crate) fn label(&self) -> String {
        let issue = self.issue.as_ref().map(|l| format!("#{}", l.number));
        let pr = self.pr.as_ref().map(|l| format!("PR #{}", l.number));
        issue.into_iter().chain(pr).collect::<Vec<_>>().join(" ")
    }
}

/// Returns the issue number a branch named `i{issue#}-{date}-{seq}` is for.
#[must_use]
pub(crate) fn issue_of(branch: &str) -> Option<u64> {
    let (number, _) = branch.strip_prefix('i')?.split_once('-')?;
    number.parse().ok()
}

/// Runs `gh` in `dir` and returns what it printed; `None` when it is
/// missing, not logged in, or `dir` is in no GitHub repository.
fn gh(dir: &Path, args: &[&str]) -> Option<String> {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    super::output("gh", dir, &args)
}

/// Parses the JSON array `gh pr list` / `gh issue list` print; anything
/// else is no rows.
fn parse_list(json: &str, kind: LinkKind) -> Vec<Link> {
    let raws: Vec<Raw> = serde_json::from_str(json).unwrap_or_default();
    raws.into_iter().filter_map(|raw| raw.link(kind)).collect()
}

/// Parses what `gh pr view` printed for a branch.
///
/// # Returns
///
/// The pull request and the first issue it closes (which may be in another
/// repository, so its own URL is kept; `gh` gives it no title).
fn parse_pr(json: &str) -> (Option<Link>, Option<Link>) {
    let Ok(mut raw) = serde_json::from_str::<Raw>(json) else {
        return (None, None);
    };
    let closes = std::mem::take(&mut raw.closes).into_iter().next();
    let closes = closes.and_then(|issue| issue.link(LinkKind::Issue));
    (raw.link(LinkKind::Pr), closes)
}

/// Reads the pull request and issue of `branch` in `dir`.
///
/// The pull request is the one GitHub has for the branch (`gh pr view`).
/// The issue is the number in the branch name ([`issue_of`]), looked up
/// with `gh issue view` (a number that is a pull request is no issue),
/// else the first issue the pull request closes.
// ponytail: `gh` has no timeout of mc's; one that hangs holds up the next
// read (not the UI). Kill it after a deadline when that is reported.
#[must_use]
pub(crate) fn read(dir: &Path, branch: &str) -> Links {
    let view = [
        "pr",
        "view",
        "--json",
        "number,title,state,url,closingIssuesReferences",
    ];
    let (pr, closes) = gh(dir, &view).map_or((None, None), |json| parse_pr(&json));
    let named = issue_of(branch).and_then(|number| {
        let number = number.to_string();
        let json = gh(dir, &["issue", "view", &number, "--json", FIELDS])?;
        let raw = serde_json::from_str::<Raw>(&json).ok()?;
        raw.link(LinkKind::Issue)
            .filter(|link| link.url.contains("/issues/"))
    });
    let issue = named.or(closes);
    Links {
        branch: branch.to_owned(),
        pr,
        issue,
    }
}

/// Lists the open pull requests, then the open issues, of the repository
/// `dir` is in; empty when `gh` could not say.
#[must_use]
pub(crate) fn list(dir: &Path) -> Vec<Link> {
    let ask = |what: &str, kind| {
        gh(dir, &[what, "list", "--json", FIELDS, "--limit", LIST_MAX])
            .map_or_else(Vec::new, |json| parse_list(&json, kind))
    };
    let mut rows = ask("pr", LinkKind::Pr);
    rows.extend(ask("issue", LinkKind::Issue));
    rows
}

/// The issues and pull requests popup (`i`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Viewer {
    /// The folder it lists for.
    pub folder: PathBuf,
    /// The rows: the session's own links first; `None` while `gh` runs.
    pub rows: Option<Vec<Link>>,
    /// How many leading rows are the session's own links.
    pub linked: usize,
    /// Highlighted row.
    pub selected: usize,
}

impl Model {
    /// Returns the folder whose links the keys act on: the one the
    /// selected session works in, else the selected project.
    fn links_folder(&self) -> Option<PathBuf> {
        self.shell_owner().map(|(_, dir)| dir)
    }

    /// Runs `P`, `I` or `i`; any other action does nothing.
    pub(super) fn link_key(&mut self, action: Action) -> Option<Cmd> {
        match action {
            Action::PullRequest => self.open_link(LinkKind::Pr),
            Action::Issue => self.open_link(LinkKind::Issue),
            Action::Links => self.open_viewer(),
            _ => None,
        }
    }

    /// Takes in the links a background read found. A read that found
    /// less for the same branch (`gh` failed: offline, rate limit) keeps
    /// what was known.
    pub(super) fn set_links(&mut self, read: Vec<(PathBuf, Links)>) {
        for (folder, mut links) in read {
            if let Some(old) = self.links.remove(&folder)
                && old.branch == links.branch
            {
                links.pr = links.pr.or(old.pr);
                links.issue = links.issue.or(old.issue);
            }
            self.links.insert(folder, links);
        }
        self.links_scan = false;
    }

    /// Opens the selected session's pull request (`P`) or issue (`I`) in
    /// the browser, or says that none is linked.
    fn open_link(&mut self, kind: LinkKind) -> Option<Cmd> {
        let link = self
            .links_folder()
            .and_then(|folder| self.links.get(&folder)?.get(kind).cloned());
        if link.is_none() {
            self.message = Some(format!(
                "No {} linked to this branch (needs the gh CLI, logged in).",
                kind.words()
            ));
        }
        link.map(|link| Cmd::OpenUrl(link.url))
    }

    /// Opens the issues and pull requests popup (`i`) for the selected
    /// session's folder and asks the loop to list them.
    fn open_viewer(&mut self) -> Option<Cmd> {
        let folder = self.links_folder()?;
        self.overlay = Some(Overlay::Links(Viewer {
            folder: folder.clone(),
            rows: None,
            linked: 0,
            selected: 0,
        }));
        Some(Cmd::ListLinks(folder))
    }

    /// Fills the open popup with what `gh` listed for `folder`: the
    /// session's own issue and pull request first, then the open ones
    /// without those two.
    pub(super) fn set_link_list(&mut self, folder: &Path, list: Vec<Link>) {
        let Some(Overlay::Links(viewer)) = &mut self.overlay else {
            return;
        };
        if viewer.folder != folder {
            return;
        }
        let own = self.links.get(folder);
        let mut rows: Vec<Link> = own
            .into_iter()
            .flat_map(|links| links.issue.iter().chain(&links.pr).cloned())
            .collect();
        viewer.linked = rows.len();
        let other = |link: &Link| {
            !rows[..viewer.linked]
                .iter()
                .any(|own| (own.kind, own.number) == (link.kind, link.number))
        };
        let rest: Vec<Link> = list.into_iter().filter(other).collect();
        rows.extend(rest);
        viewer.rows = Some(rows);
    }

    /// Applies a key to the popup: `j`/`k` or `↑`/`↓` move, `enter` opens
    /// the row in the browser (the popup stays), `esc` or `q` closes.
    pub(super) fn viewer_key(&mut self, mut viewer: Viewer, key: KeyEvent) -> Option<Cmd> {
        let last = viewer
            .rows
            .as_ref()
            .map_or(0, |r| r.len().saturating_sub(1));
        let cmd = match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return None,
            KeyCode::Up | KeyCode::Char('k') => {
                viewer.selected = viewer.selected.saturating_sub(1);
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                viewer.selected = (viewer.selected + 1).min(last);
                None
            }
            KeyCode::Enter => viewer
                .rows
                .as_ref()
                .and_then(|rows| rows.get(viewer.selected))
                .map(|link| Cmd::OpenUrl(link.url.clone())),
            _ => None,
        };
        self.overlay = Some(Overlay::Links(viewer));
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppEvent;
    use crate::app::model::tests::{press, sample, with_session};

    const PR: &str = r#"{"number":189,"title":"feat: open \u001b[31mthe PR","state":"OPEN",
        "url":"https://github.com/osbrjp/bungkus-mc/pull/189",
        "closingIssuesReferences":[{"number":188,"url":"https://github.com/osbrjp/bungkus-mc/issues/188"}]}"#;

    fn link(kind: LinkKind, number: u64) -> Link {
        Link {
            kind,
            number,
            title: format!("title {number}"),
            state: "open".into(),
            url: format!("https://github.com/o/r/x/{number}"),
        }
    }

    #[test]
    fn reads_the_issue_number_from_the_branch_name() {
        for (branch, want) in [
            ("i188-20261002-2031", Some(188)),
            ("i12-x", Some(12)),
            ("main", None),
            ("i-20261002", None),
            ("idea-3", None),
            ("i188", None),
        ] {
            assert_eq!(issue_of(branch), want, "{branch}");
        }
    }

    #[test]
    fn parses_gh_output_and_keeps_only_plain_https_urls() {
        let (pr, closes) = parse_pr(PR);
        let pr = pr.unwrap();
        assert_eq!((pr.number, closes.map(|l| l.number)), (189, Some(188)));
        assert_eq!(
            (pr.title.as_str(), pr.state.as_str()),
            ("feat: open the PR", "open")
        );
        assert_eq!(parse_pr("no pull requests found"), (None, None));

        let list = r#"[{"number":1,"url":"https://github.com/o/r/issues/1"},
            {"number":2,"url":"file:///etc/passwd"},
            {"number":3,"url":"--help"},
            {"number":4,"url":"https://github.com/o/r/issues/4 -x"}]"#;
        let numbers: Vec<u64> = parse_list(list, LinkKind::Issue)
            .iter()
            .map(|l| l.number)
            .collect();
        assert_eq!(numbers, [1]);
        assert!(parse_list("{}", LinkKind::Pr).is_empty());
    }

    #[test]
    fn the_card_label_names_the_issue_and_the_pull_request() {
        let mut links = Links::default();
        assert_eq!(links.label(), "");
        links.pr = Some(link(LinkKind::Pr, 189));
        assert_eq!(links.label(), "PR #189");
        links.issue = Some(link(LinkKind::Issue, 188));
        assert_eq!(links.label(), "#188 PR #189");
    }

    #[test]
    fn shift_p_and_shift_i_open_the_sessions_links_or_say_there_are_none() {
        let mut m = sample(&["a"]);
        let (_id, _writes) = with_session(&mut m, "one");
        m.focus = crate::app::model::Focus::Sessions;
        assert_eq!(m.update(press(KeyCode::Char('P'))), None);
        assert!(m.message.as_deref().unwrap().starts_with("No pull request"));
        let folder = m.cards[0].folder();
        let links = Links {
            branch: "i188-x".into(),
            pr: Some(link(LinkKind::Pr, 189)),
            issue: Some(link(LinkKind::Issue, 188)),
        };
        m.update(AppEvent::Links(vec![(folder.clone(), links)]));
        let failed = Links {
            branch: "i188-x".into(),
            ..Links::default()
        };
        m.update(AppEvent::Links(vec![(folder.clone(), failed)]));
        for (key, number) in [('P', 189), ('I', 188)] {
            assert_eq!(
                m.update(press(KeyCode::Char(key))),
                Some(Cmd::OpenUrl(format!("https://github.com/o/r/x/{number}")))
            );
        }
        let repo = crate::app::repo::Status::parse("# branch.head i188-x\n").unwrap();
        m.repos.insert(folder, repo);
        let screen = crate::ui::tests::render(&mut m, 120, 40);
        assert!(screen.contains("i188-x · clean · #188 PR #189"), "{screen}");
    }

    #[test]
    fn i_lists_the_sessions_links_first_and_enter_opens_the_selected_row() {
        let mut m = sample(&["a"]);
        let (_id, _writes) = with_session(&mut m, "one");
        m.focus = crate::app::model::Focus::Sessions;
        let folder = m.cards[0].folder();
        let links = Links {
            branch: "i188-x".into(),
            pr: Some(link(LinkKind::Pr, 189)),
            issue: Some(link(LinkKind::Issue, 188)),
        };
        m.update(AppEvent::Links(vec![(folder.clone(), links)]));
        assert_eq!(
            m.update(press(KeyCode::Char('i'))),
            Some(Cmd::ListLinks(folder.clone()))
        );
        assert!(crate::ui::tests::render(&mut m, 120, 40).contains("asking gh"));
        let listed = vec![
            link(LinkKind::Pr, 189),
            link(LinkKind::Pr, 7),
            link(LinkKind::Issue, 188),
            link(LinkKind::Issue, 5),
        ];
        m.update(AppEvent::LinkList(folder, listed));
        let Some(Overlay::Links(viewer)) = &m.overlay else {
            panic!("i opens the popup");
        };
        let rows: Vec<u64> = viewer.rows.iter().flatten().map(|l| l.number).collect();
        assert_eq!((rows, viewer.linked), (vec![188, 189, 7, 5], 2));
        let screen = crate::ui::tests::render(&mut m, 120, 40);
        assert!(screen.contains("#189") && screen.contains("title 5"));

        m.update(press(KeyCode::Char('j')));
        m.update(press(KeyCode::Down));
        assert_eq!(
            m.update(press(KeyCode::Enter)),
            Some(Cmd::OpenUrl("https://github.com/o/r/x/7".into()))
        );
        assert!(m.overlay.is_some(), "the popup stays for the next row");
        m.update(press(KeyCode::Esc));
        assert!(m.overlay.is_none());
    }
}
