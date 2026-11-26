//! The HTML report, made of the results there are.

use std::{
    collections::{BTreeMap, HashMap},
    fmt::{self, Display},
};

use askama::Template;
use camino::Utf8Path;
use jiff::Timestamp;
use percent_encoding::{AsciiSet, CONTROLS, NON_ALPHANUMERIC, utf8_percent_encode};

use super::{File, Kind, Line, LineOutcome, Stored, Taking, Verdict, output::Reported};
use crate::diagnostic::{Highlighted, diff};

/// What every report has near its start, and no other page.
pub(super) const TITLE_ENDS: &str = "· catalog tests</title>";

#[derive(Template)]
#[template(path = "page.html")]
struct Page<'a> {
    styles: &'static str,
    script: &'static str,
    /// What it all came to, in a few words.
    summary: String,
    /// The same as a data URI.
    favicon: String,
    failed: usize,
    unfinished: usize,
    passed: usize,
    /// Of those passed.
    tolerated: usize,
    earlier: usize,
    catalogs: Vec<Catalog<'a>>,
}

/// Gallery's mark, on the colour of a verdict.
#[derive(Template)]
#[template(path = "favicon.svg")]
struct Favicon {
    colour: &'static str,
}

impl Favicon {
    fn of(verdict: Verdict) -> String {
        let icon = Self {
            colour: match verdict {
                Verdict::Passed => "#1a7a42",
                Verdict::Unfinished => "#9a6200",
                Verdict::Failed => "#c22f36",
            },
        };
        let svg = icon.render().expect("BUG: a template writes to a string");
        let encoded = utf8_percent_encode(svg.trim(), RESERVED);
        format!("data:image/svg+xml,{encoded}")
    }
}

/// One crate's tests on one renderer.
struct Catalog<'a> {
    id: String,
    krate: &'a str,
    renderer: &'a str,
    ran: Time,
    scenes: Vec<Scene<'a>>,
}

struct Scene<'a> {
    id: String,
    key: &'a str,
    /// The modules it is in, without its crate.
    module: String,
    name: &'a str,
    /// The worst of its tests'.
    verdict: Verdict,
    /// All of its tests together.
    took_ms: u64,
    trials: Vec<Trial<'a>>,
}

impl<'a> Scene<'a> {
    /// Its one test, when that only drew it.
    fn only(&self) -> Option<&Trial<'a>> {
        match &self.trials[..] {
            [trial] if trial.plain => Some(trial),
            _ => None,
        }
    }

    fn took(&self) -> String {
        took(self.took_ms)
    }
}

struct Trial<'a> {
    id: String,
    /// What the page's styles and filters go by.
    classes: String,
    name: &'a str,
    /// There is nothing to say of it
    /// but that its scene was drawn.
    plain: bool,
    verdict: Verdict,
    /// When it ran, if that was before its catalog's latest run.
    earlier: Option<Time>,
    took: String,
    warned: bool,
    tolerated: bool,
    /// A [`Body`], as it was rendered when the test was over.
    body: &'a str,
}

/// What is said of a test when it is looked at.
#[derive(Template)]
#[template(path = "body.html")]
struct Body<'a> {
    unfinished: bool,
    warnings: &'a [String],
    video: Option<String>,
    lines: Vec<LogLine>,
    failure: Option<Vec<Piece>>,
}

/// Text of a failure or a diff, and the classes saying what it is, if anything.
struct Piece {
    text: String,
    class: Option<String>,
}

struct LogLine {
    depth: usize,
    kind: Kind,
    text: String,
    found: Option<String>,
    notes: Vec<String>,
    outcome: LineOutcome,
    frames: u32,
    /// The line of the play that failed, if it did.
    source: Option<String>,
    seek: Option<Seek>,
    images: Vec<Link>,
    texts: Vec<Link>,
    diff: Vec<DiffLine>,
    /// What became of what it saw, as a reference to be.
    taking: Option<Taking>,
}

/// Where in the video a line of the log begins.
struct Seek {
    /// The video from there on.
    href: String,
    /// In seconds.
    at: String,
    shown: String,
}

impl Seek {
    fn new(video: &str, seconds: f64) -> Self {
        Self {
            href: format!("{video}#t={seconds:.3}"),
            at: format!("{seconds:.3}"),
            shown: format!("{seconds:.1} s"),
        }
    }
}

struct Link {
    name: String,
    href: String,
}

struct DiffLine {
    kind: Difference,
    sign: String,
    pieces: Vec<Piece>,
}

enum Difference {
    Removed,
    Added,
    None,
}

struct Time {
    /// For the page's script, which shows it as the reader's own.
    rfc3339: String,
    /// For whoever reads the page without it.
    utc: String,
}

impl From<Timestamp> for Time {
    fn from(at: Timestamp) -> Self {
        Self {
            // HTML's times have three digits of a second's fraction at most.
            rfc3339: at.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            utc: at.strftime("%Y-%m-%d %H:%M UTC").to_string(),
        }
    }
}

/// As the page's styles name it.
impl Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Passed => "passed",
            Self::Unfinished => "unfinished",
            Self::Failed => "failed",
        })
    }
}

/// As the page's styles name it.
impl Display for LineOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unfinished => "unfinished",
            Self::Passed => "passed",
            Self::Tolerated => "tolerated",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        })
    }
}

/// As the page's styles name it.
impl Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Step => "step",
            Self::Gesture => "gesture",
            Self::Check => "check",
        })
    }
}

/// As the page's styles name it.
impl Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Removed => "removed",
            Self::Added => "added",
            Self::None => "alike",
        })
    }
}

impl LogLine {
    /// There is more to say of it than fits on it.
    fn has_more(&self) -> bool {
        self.outcome != LineOutcome::Passed
            || self.taking.is_some()
            || !self.notes.is_empty()
            || !self.images.is_empty()
            || !self.texts.is_empty()
    }
}

/// A test's part of the report.
pub(super) fn body(stored: &Stored) -> String {
    let video = stored.video.as_deref().map(href);
    let log = stored.log.iter();
    let body = Body {
        unfinished: stored.kept.verdict == Verdict::Unfinished,
        warnings: &stored.warnings,
        lines: log.map(|logged| line(logged, video.as_deref())).collect(),
        video,
        failure: stored.failure.as_ref().map(pieces),
    };
    body.render().expect("BUG: a template writes to a string")
}

/// `failure` cut at its tokens, or whole where they don't fit it, as in a result edited by hand.
fn pieces(failure: &Highlighted) -> Vec<Piece> {
    let text = failure.text.as_str();
    let plain = |text: &str| Piece {
        text: text.to_owned(),
        class: None,
    };
    let mut pieces = Vec::with_capacity(failure.tokens.len() * 2 + 1);
    let mut at = 0;
    for token in &failure.tokens {
        let (Some(before), Some(inside)) =
            (text.get(at..token.start), text.get(token.start..token.end))
        else {
            return vec![plain(text)];
        };
        pieces.push(plain(before));
        pieces.push(Piece {
            text: inside.to_owned(),
            class: Some(token.kind.class().to_owned()),
        });
        at = token.end;
    }
    pieces.push(plain(&text[at..]));
    pieces
}

/// The lines of a snapshot's `diff`, each piece classed by its token and whether its words changed.
fn diff_lines(diff: &str) -> Vec<DiffLine> {
    let lines = diff::lines(diff, diff::SNAPSHOTS).into_iter();
    lines
        .map(|line| DiffLine {
            kind: match line.change {
                diff::Change::Removed => Difference::Removed,
                diff::Change::Added => Difference::Added,
                diff::Change::Kept | diff::Change::Elided => Difference::None,
            },
            sign: line.sign.to_owned(),
            pieces: line
                .pieces
                .iter()
                .map(|piece| {
                    let token = piece.kind.map(|kind| kind.class());
                    let changed = piece.changed.then_some("changed");
                    let classes: Vec<&str> = token.into_iter().chain(changed).collect();
                    Piece {
                        text: piece.text.to_owned(),
                        class: (!classes.is_empty()).then(|| classes.join(" ")),
                    }
                })
                .collect(),
        })
        .collect()
}

/// The report of `results`.
pub(super) fn render(results: &[Reported]) -> String {
    let mut catalogs: BTreeMap<(&str, &str), Vec<&Reported>> = BTreeMap::new();
    for reported in results {
        let kept = &reported.kept;
        let catalog = (kept.krate.as_str(), kept.renderer.as_str());
        catalogs.entry(catalog).or_default().push(reported);
    }
    let catalogs: Vec<Catalog<'_>> = catalogs
        .into_iter()
        .map(|((krate, renderer), results)| catalog(krate, renderer, results))
        .collect();
    let count = |is: &dyn Fn(&Trial<'_>) -> bool| {
        let scenes = catalogs.iter().flat_map(|catalog| &catalog.scenes);
        scenes
            .flat_map(|scene| &scene.trials)
            .filter(|trial| is(trial))
            .count()
    };
    let failed = count(&|trial| trial.verdict == Verdict::Failed);
    let unfinished = count(&|trial| trial.verdict == Verdict::Unfinished);
    let page = Page {
        // Not included by the template, which would read JSDoc's `{{ … }}` as its own.
        styles: include_str!("../../templates/page.css"),
        script: include_str!("../../templates/page.js"),
        summary: match (failed, unfinished) {
            (0, 0) => "passed".to_owned(),
            (0, n) => format!("{n} unfinished"),
            (n, _) => format!("{n} failed"),
        },
        favicon: Favicon::of(match (failed, unfinished) {
            (0, 0) => Verdict::Passed,
            (0, _) => Verdict::Unfinished,
            _ => Verdict::Failed,
        }),
        failed,
        unfinished,
        passed: count(&|trial| trial.verdict == Verdict::Passed),
        tolerated: count(&|trial| trial.tolerated),
        earlier: count(&|trial| trial.earlier.is_some()),
        catalogs,
    };
    page.render().expect("BUG: a template writes to a string")
}

fn catalog<'a>(krate: &'a str, renderer: &'a str, mut results: Vec<&'a Reported>) -> Catalog<'a> {
    results.sort_by(|a, b| (a.order, &a.kept.trial).cmp(&(b.order, &b.kept.trial)));
    let latest = results.iter().max_by_key(|reported| reported.kept.at);
    let latest = &latest.expect("BUG: a catalog there is no result of").kept;
    let id = anchor(&format!("{krate}.{renderer}"));
    let mut scenes: Vec<Scene<'a>> = Vec::new();
    let mut places: HashMap<&str, usize> = HashMap::new();
    for reported in &results {
        let trial = trial(&id, reported, &latest.run);
        let key = reported.kept.scene.as_str();
        match places.get(key) {
            Some(place) => {
                let scene = &mut scenes[*place];
                scene.verdict = scene.verdict.max(trial.verdict);
                scene.took_ms = scene.took_ms.saturating_add(reported.kept.took_ms);
                scene.trials.push(trial);
            }
            None => {
                let mut path: Vec<&str> = key.split("::").collect();
                let name = path.pop().unwrap_or_default();
                let module = path.into_iter().skip(1).collect::<Vec<_>>();
                places.insert(key, scenes.len());
                scenes.push(Scene {
                    // Apart from a test's, which begins with `smoke::` or `play::`.
                    id: format!("{id}.scene::{}", anchor(key)),
                    key,
                    module: module.join("::"),
                    name,
                    verdict: trial.verdict,
                    took_ms: reported.kept.took_ms,
                    trials: vec![trial],
                });
            }
        }
    }
    Catalog {
        id,
        krate,
        renderer,
        ran: latest.at.into(),
        scenes,
    }
}

fn trial<'a>(catalog: &str, reported: &'a Reported, run: &str) -> Trial<'a> {
    let kept = &reported.kept;
    let earlier = kept.run != run;
    Trial {
        id: format!("{catalog}.{}", anchor(&kept.trial)),
        classes: format!(
            "trial {}{}{}",
            kept.verdict,
            if earlier { " earlier" } else { "" },
            if kept.tolerated { " tolerated" } else { "" }
        ),
        name: kept.play.as_deref().unwrap_or("drawn"),
        plain: kept.play.is_none() && kept.verdict == Verdict::Passed && !kept.warned,
        verdict: kept.verdict,
        earlier: earlier.then(|| kept.at.into()),
        took: took(kept.took_ms),
        warned: kept.warned,
        tolerated: kept.tolerated,
        body: &reported.body,
    }
}

fn line(line: &Line, video: Option<&str>) -> LogLine {
    let (images, texts) = line
        .files
        .iter()
        .partition::<Vec<_>, _>(|file| file.path.extension() == Some("png"));
    let links = |files: Vec<&File>| {
        files
            .into_iter()
            .map(|file| Link {
                name: file.name.clone(),
                href: href(&file.path),
            })
            .collect()
    };
    LogLine {
        depth: line.depth,
        kind: line.kind,
        text: line.text.clone(),
        found: line.found.clone(),
        notes: line.notes.clone(),
        outcome: line.outcome,
        frames: line.frames,
        source: line
            .source
            .clone()
            .filter(|_| line.outcome == LineOutcome::Failed),
        seek: video.zip(line.seek).map(|(video, at)| Seek::new(video, at)),
        images: links(images),
        texts: links(texts),
        diff: diff_lines(line.diff.as_deref().unwrap_or_default()),
        taking: line.acceptance,
    }
}

/// All but what RFC 3986 reserves nothing for.
const RESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// A path in the output as a link from its root.
fn href(path: &Utf8Path) -> String {
    path.components()
        .map(|part| utf8_percent_encode(part.as_str(), RESERVED).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// `name` as an anchor, and as a link to it has it: RFC 3986's fragment, which takes `::`.
fn anchor(name: &str) -> String {
    const RESERVED: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'%')
        .add(b'<')
        .add(b'>')
        .add(b'[')
        .add(b'\\')
        .add(b']')
        .add(b'^')
        .add(b'`')
        .add(b'{')
        .add(b'|')
        .add(b'}');
    utf8_percent_encode(name, RESERVED).to_string()
}

fn took(ms: u64) -> String {
    match ms {
        0..1_000 => format!("{ms} ms"),
        _ => format!("{}.{} s", ms / 1_000, ms % 1_000 / 100),
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

    use crate::diagnostic::{Highlighted, Kind as TokenKind, Token};

    use super::{
        super::{Kept, VERSION},
        *,
    };

    fn stored(krate: &str, trial: &str, verdict: Verdict) -> Stored {
        let parts: Vec<&str> = trial.split("::").skip(1).collect();
        let (scene, play) = match trial.strip_prefix("play::") {
            Some(_) => (&parts[..parts.len() - 1], parts.last()),
            None => (&parts[..], None),
        };
        Stored {
            kept: Kept {
                gallery: VERSION.to_owned(),
                krate: krate.to_owned(),
                manifest_dir: Utf8PathBuf::new(),
                renderer: "glow".to_owned(),
                run: "1".to_owned(),
                at: "2026-11-25T12:00:00Z".parse().expect("a time"),
                trial: trial.to_owned(),
                scene: scene.join("::"),
                play: play.map(|play| (*play).to_owned()),
                verdict,
                took_ms: 0,
                warned: false,
                tolerated: false,
            },
            failure: None,
            warnings: Vec::new(),
            log: Vec::new(),
            video: None,
        }
    }

    /// The report of `results`, in their catalog's order.
    fn render(results: &[Stored]) -> String {
        let reported = results.iter().enumerate().map(|(order, stored)| Reported {
            kept: stored.kept.clone(),
            order,
            body: body(stored),
        });
        super::render(&reported.collect::<Vec<_>>())
    }

    fn logged(outcome: LineOutcome) -> Line {
        Line {
            depth: 0,
            kind: Kind::Check,
            text: "button `Save` exists".to_owned(),
            found: None,
            notes: Vec::new(),
            outcome,
            frames: 1,
            source: Some("plays.scene.rs:7".to_owned()),
            seek: None,
            files: Vec::new(),
            diff: None,
            acceptance: None,
        }
    }

    /// The page of a play with this line in its log.
    fn page_of(line: Line) -> String {
        let mut played = stored("plays", "play::plays::Todo::adds", Verdict::Failed);
        played.log.push(line);
        played.video = Some("plays/adds/video.glow.mp4".into());
        render(&[played])
    }

    /// The text between the first `start` and the next `end`.
    fn between<'p>(page: &'p str, start: &str, end: &str) -> &'p str {
        let (_, rest) = page.split_once(start).expect("a start");
        rest.split_once(end).expect("an end").0
    }

    fn filter(page: &str, name: &str) -> usize {
        let label = between(page, &format!("<label for=\"show-{name}\""), "</label>");
        between(label, "<b>", "</b>").parse().expect("a number")
    }

    #[test]
    fn a_report_is_told_by_its_start() {
        let page = render(&[]);
        assert!(page.find(TITLE_ENDS).is_some_and(|at| at < 2_048));
        assert!(page.contains("No test has run yet."));
    }

    #[test]
    fn the_title_and_the_filters_count_the_verdicts() {
        use Verdict::{Failed, Passed, Unfinished};
        let trial = |verdict, n| stored("plays", &format!("smoke::plays::{n}"), verdict);
        let some = [
            trial(Failed, 0),
            trial(Failed, 1),
            trial(Unfinished, 2),
            trial(Passed, 3),
        ];
        let page = render(&some);
        assert!(page.contains("<title>2 failed ·"), "{page}");
        let counts = ["all", "failed", "unfinished", "passed"].map(|name| filter(&page, name));
        assert_eq!(counts, [4, 2, 1, 1]);
        let title = |some: &[Stored]| between(&render(some), "<title>", " ·").to_owned();
        assert_eq!(title(&some[2..]), "1 unfinished");
        assert_eq!(title(&some[3..]), "passed");
    }

    #[test]
    fn a_test_not_run_since_says_when_it_was() {
        let mut earlier = [
            stored("plays", "smoke::plays::Todo", Verdict::Passed),
            stored("plays", "play::plays::Todo::adds", Verdict::Passed),
        ];
        for stored in &mut earlier {
            stored.kept.run = "0".to_owned();
            stored.kept.at = "2026-11-24T08:30:00Z".parse().expect("a time");
        }
        let [smoke, play] = earlier;
        let page = render(&[
            smoke,
            play,
            stored("plays", "smoke::plays::Other", Verdict::Passed),
        ]);
        let said =
            "not run since <time datetime=\"2026-11-24T08:30:00.000Z\">2026-11-24 08:30 UTC</time>";
        assert_eq!(page.matches(said).count(), 2, "{page}");
        assert_eq!(filter(&page, "earlier"), 2);
        let ran = between(&page, "<h2>", "</h2>");
        assert!(ran.contains("2026-11-25 12:00 UTC"), "the latest: {ran}");
    }

    #[test]
    fn what_is_said_of_a_drawn_scene_not_run_since_is_filtered_with_it() {
        let mut earlier = stored("plays", "smoke::plays::Todo", Verdict::Passed);
        earlier.kept.run = "0".to_owned();
        earlier.kept.at = "2026-11-24T08:30:00Z".parse().expect("a time");
        let page = render(&[
            earlier,
            stored("plays", "smoke::plays::Other", Verdict::Passed),
        ]);
        let scene = between(&page, "<section class=\"scene", "</section>");
        let since = between(
            scene,
            "<span class=\"since trial passed earlier\">",
            "</h3>",
        );
        assert!(since.contains("not run since"), "{since}");
    }

    #[test]
    fn a_play_that_passed_keeps_its_log_and_video_closed() {
        let mut passing = stored("plays", "play::plays::Todo::adds", Verdict::Passed);
        passing.log.push(logged(LineOutcome::Passed));
        passing.video = Some("plays/adds/video.glow.mp4".into());
        let page = render(&[passing]);
        let tag = between(&page, "<details", ">");
        assert!(!tag.contains("open"), "{tag}");
        let body = between(&page, "<div class=\"body\">", "</details>");
        assert!(body.contains("button `Save` exists"), "{body}");
        assert!(!body.contains("did not finish"), "{body}");
        let video = between(body, "<video", ">");
        for attribute in [
            "controls",
            "preload=\"none\"",
            "src=\"plays/adds/video.glow.mp4\"",
        ] {
            assert!(video.contains(attribute), "{video}");
        }
    }

    #[test]
    fn a_test_that_did_not_pass_is_open() {
        for verdict in [Verdict::Failed, Verdict::Unfinished] {
            let page = render(&[stored("plays", "play::plays::Todo::adds", verdict)]);
            let tag = between(&page, "<details", ">");
            assert!(tag.contains("open"), "{tag}");
            let unfinished = verdict == Verdict::Unfinished;
            assert_eq!(page.contains("did not finish"), unfinished, "{verdict}");
        }
    }

    #[test]
    fn a_test_that_warned_says_so_before_it_is_opened() {
        let mut warned = stored("plays", "smoke::plays::Todo", Verdict::Passed);
        let summary = |page: &str| between(page, "<summary>", "</summary>").to_owned();
        warned.kept.warned = true;
        assert!(summary(&render(&[warned])).contains(">warned</span>"));
        let failed = stored("plays", "smoke::plays::Todo", Verdict::Failed);
        assert!(!summary(&render(&[failed])).contains("warned"));
    }

    #[test]
    fn what_was_only_drawn_is_in_the_minimap_by_its_scene_alone() {
        let page = render(&[
            stored("plays", "smoke::plays::Todo", Verdict::Passed),
            stored("plays", "play::plays::Todo::adds", Verdict::Passed),
        ]);
        let map = between(&page, "<nav", "</nav>");
        let link = |of: &str| between(map, &format!("href=\"#plays.glow.{of}\""), ">").to_owned();
        assert!(link("smoke::plays::Todo").contains("hidden"));
        assert!(!link("play::plays::Todo::adds").contains("hidden"));
    }

    #[test]
    fn a_filter_has_the_minimap_lose_the_catalogs_it_hides() {
        let page = render(&[
            stored("plays", "smoke::plays::Todo", Verdict::Passed),
            stored("other", "smoke::other::Todo", Verdict::Failed),
        ]);
        let map = between(&page, "<nav", "</nav>");
        for part in map.split("<section class=\"catalog\">").skip(1) {
            let part = part.split("</section>").next().expect("its end");
            assert_eq!(part.matches("class=\"trial ").count(), 1, "{part}");
        }
        assert_eq!(map.matches("<section class=\"catalog\">").count(), 2);
    }

    #[test]
    fn the_page_has_its_script() {
        let page = render(&[]);
        assert!(page.contains(include_str!("../../templates/page.js")));
    }

    #[test]
    fn a_line_that_passed_says_what_was_noted_of_it() {
        let mut line = logged(LineOutcome::Passed);
        line.notes.push("two were added".to_owned());
        assert!(page_of(line).contains("<p class=\"note\">two were added</p>"));
    }

    #[test]
    fn a_scene_shows_the_worst_of_its_tests_their_time_and_its_module_without_its_crate() {
        use Verdict::{Failed, Passed};
        let mut tests = [
            stored("plays", "smoke::plays::lists::Todo", Passed),
            stored("plays", "play::plays::lists::Todo::adds", Failed),
            stored("plays", "play::plays::lists::Todo::sorts", Passed),
        ];
        for test in &mut tests {
            test.kept.took_ms = 700;
        }
        let page = render(&tests);
        let scene = between(&page, "<section class=\"scene", "</h3>");
        assert!(scene.starts_with(" failed\""), "{scene}");
        assert!(scene.contains("failed:</span>"), "{scene}");
        assert!(scene.contains(">2.1 s<"), "{scene}");
        let path = between(&page, "<span class=\"path\">", "</h3>");
        assert!(path.contains("lists::"), "{path}");
        assert!(!path.contains("plays::"), "{path}");
    }

    #[test]
    fn tests_that_say_they_took_for_ever_took_no_longer_together() {
        let mut tests = ["adds", "sorts"].map(|play| {
            let play = format!("play::plays::Todo::{play}");
            stored("plays", &play, Verdict::Passed)
        });
        for test in &mut tests {
            test.kept.took_ms = u64::MAX;
        }
        render(&tests);
    }

    #[test]
    fn the_tabs_icon_tells_a_failed_run_from_an_unfinished_and_a_passed_one() {
        use Verdict::{Failed, Passed, Unfinished};
        let icon = |verdicts: &[Verdict]| {
            let tests = verdicts.iter().enumerate();
            let tests: Vec<Stored> = tests
                .map(|(n, verdict)| stored("plays", &format!("smoke::plays::{n}"), *verdict))
                .collect();
            let page = render(&tests);
            let icon = between(&page, "<link rel=\"icon\"", ">");
            let uri = between(icon, "href=\"data:image/svg+xml,", "\"");
            let svg = percent_encoding::percent_decode_str(uri).decode_utf8();
            let svg = svg.expect("UTF-8").into_owned();
            between(&svg, "<rect", "/>").to_owned()
        };
        let passed = icon(&[Passed, Passed]);
        let unfinished = icon(&[Passed, Unfinished]);
        let failed = icon(&[Unfinished, Failed, Passed]);
        assert!(passed != unfinished && unfinished != failed && failed != passed);
        assert!(failed.contains("fill=\"#c22f36\""), "{failed}");
    }

    #[test]
    fn a_scene_that_is_only_drawn_is_one_line() {
        use Verdict::Passed;
        let drawn = stored("plays", "smoke::plays::Todo", Passed);
        let alone = render(std::slice::from_ref(&drawn));
        assert!(!alone.contains("class=\"trials\""), "{alone}");
        assert!(alone.contains("<span class=\"plain trial passed\""));
        let played = stored("plays", "play::plays::Todo::adds", Passed);
        let both = render(&[drawn, played]);
        let trials = between(&both, "class=\"trials\"", "</section>");
        assert!(trials.contains(">drawn</span>"), "{trials}");
        assert!(trials.contains(">adds</span>"), "{trials}");
    }

    #[test]
    fn tests_come_in_their_catalogs_order() {
        let trial = |name: &str, order| Reported {
            kept: stored("plays", &format!("smoke::plays::{name}"), Verdict::Passed).kept,
            order,
            body: String::new(),
        };
        let unlisted = usize::MAX;
        let page = super::render(&[
            trial("bee", unlisted),
            trial("ape", unlisted),
            trial("ant", 2),
            trial("moth", 1),
            trial("zebra", 0),
        ]);
        let order = ["zebra", "moth", "ant", "ape", "bee"];
        let at = order.map(|name| page.find(&format!("id=\"plays.glow.smoke::plays::{name}\"")));
        assert!(at.is_sorted() && at.iter().all(Option::is_some), "{at:?}");
    }

    #[test]
    fn no_two_things_have_one_anchor() {
        let mut all = Vec::new();
        for krate in ["smoke", "play", "scene"] {
            for trial in ["smoke::{}::a", "play::{}::a::b", "smoke::{}::{}::a"] {
                let trial = trial.replace("{}", krate);
                all.push(stored(krate, &trial, Verdict::Passed));
            }
        }
        let page = render(&all);
        let mut ids: Vec<&str> = page
            .match_indices(" id=\"")
            .map(|(at, _)| between(&page[at..], " id=\"", "\""))
            .collect();
        let all = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all);
    }

    #[test]
    fn a_link_in_the_minimap_says_its_verdict_and_leads_somewhere() {
        let page = render(&[
            stored("plays", "smoke::plays::Todo", Verdict::Passed),
            stored("plays", "play::plays::Todo::adds", Verdict::Failed),
            stored("plays", "play::plays::Todo::a/b", Verdict::Unfinished),
        ]);
        let map = between(&page, "<nav", "</nav>");
        for (link, verdict) in [("adds", "failed"), ("a/b", "unfinished")] {
            let at = map.find(&format!("::{link}\"")).expect("a link");
            let link = between(&map[at..], ">", "</a>");
            assert!(link.contains(&format!("{verdict}:</span>")), "{link}");
        }
        for (at, _) in map.match_indices("href=\"#") {
            let id = between(&map[at..], "href=\"#", "\"");
            assert!(page.contains(&format!(" id=\"{id}\"")), "{id}");
        }
    }

    #[test]
    fn what_was_found_is_got_by_a_failed_check_alone() {
        let found = |outcome| {
            let mut line = logged(outcome);
            line.found = Some("not compared in a capture".to_owned());
            page_of(line)
        };
        assert!(found(LineOutcome::Failed).contains("got not compared"));
        let skipped = found(LineOutcome::Skipped);
        assert!(skipped.contains("not compared"), "{skipped}");
        assert!(!skipped.contains("got not compared"), "{skipped}");
    }

    #[test]
    fn a_tolerated_check_is_marked_says_what_differed_and_has_its_test_counted_apart() {
        let mut line = logged(LineOutcome::Tolerated);
        line.found = Some("12 pixels differ, 50 allowed".to_owned());
        let mut played = stored("plays", "play::plays::Todo::adds", Verdict::Passed);
        played.log.push(line);
        played.kept.tolerated = true;
        let page = render(&[
            played,
            stored("plays", "smoke::plays::Todo", Verdict::Passed),
        ]);
        assert!(
            page.contains("<span class=\"found\">12 pixels differ, 50 allowed</span>"),
            "{page}"
        );
        assert!(page.contains("<li class=\"check tolerated\""), "{page}");
        let summary = between(&page, "<summary>", "</summary>");
        assert!(summary.contains(">tolerated</span>"), "{summary}");
        assert_eq!(filter(&page, "tolerated"), 1);
        assert_eq!(
            filter(&page, "passed"),
            2,
            "a tolerated test passed all the same"
        );
        let untouched = render(&[stored("plays", "smoke::plays::Todo", Verdict::Passed)]);
        assert!(
            !untouched.contains("id=\"show-tolerated\""),
            "no filter for none"
        );
    }

    #[test]
    fn a_line_that_failed_says_where_in_its_source() {
        for (outcome, says) in [
            (LineOutcome::Failed, true),
            (LineOutcome::Passed, false),
            (LineOutcome::Skipped, false),
        ] {
            let page = page_of(logged(outcome));
            assert_eq!(page.contains("plays.scene.rs:7"), says, "{outcome:?}");
        }
    }

    /// What the page shows of a test that failed with `text` and its `tokens`.
    fn failure_shown(text: &str, tokens: Vec<Token>) -> String {
        let mut failed = stored("plays", "play::plays::Todo::adds", Verdict::Failed);
        failed.failure = Some(Highlighted {
            text: text.to_owned(),
            tokens,
        });
        let page = render(&[failed]);
        between(&page, "<pre class=\"failure\">", "</pre>").to_owned()
    }

    fn token(start: usize, end: usize, kind: TokenKind) -> Token {
        Token { start, end, kind }
    }

    #[test]
    fn a_failures_code_is_coloured_by_what_each_token_is_and_escaped() {
        let text = "× it failed\n3 │ let x = \"<b>\"; // é";
        let at = |part: &str| text.find(part).expect("in the text");
        let shown = failure_shown(
            text,
            vec![
                token(at("let"), at("let") + 3, TokenKind::Keyword),
                token(at("\"<b>\""), at(";"), TokenKind::String),
                token(at("//"), text.len(), TokenKind::Comment),
            ],
        );
        assert!(
            shown.starts_with("× it failed\n3 │ <span class=\"keyword\">let</span> x = "),
            "{shown}"
        );
        let string = between(&shown, "<span class=\"string\">", "</span>");
        assert!(
            string.contains('b') && !string.contains(['<', '"']),
            "{string}"
        );
        assert!(
            shown.ends_with("<span class=\"comment\">// é</span>"),
            "{shown}"
        );
    }

    #[test]
    fn a_snapshots_diff_is_classed_by_its_tokens_and_the_words_that_changed() {
        let mut line = logged(LineOutcome::Failed);
        line.diff = Some(" {\n-  \"on\": \"<no>\"\n+  \"on\": \"yes\"\n…\n".to_owned());
        let page = page_of(line);
        let diff = between(&page, "<pre class=\"diff\">", "</pre>");
        let lines: Vec<&str> = diff.split("<span class=\"").skip(1).collect();
        assert!(
            diff.starts_with("<span class=\"alike\"><span class=\"sign\"> </span>{</span>"),
            "{diff}"
        );
        assert!(
            diff.contains("<span class=\"removed\"><span class=\"sign\">-</span>  <span class=\"string\">&#34;on&#34;</span>: <span class=\"string changed\">&#34;&#60;no&#62;&#34;</span></span>"),
            "{lines:?}"
        );
        assert!(
            diff.contains("<span class=\"string changed\">&#34;yes&#34;</span>"),
            "{diff}"
        );
        assert!(
            diff.ends_with("<span class=\"alike\"><span class=\"sign\">…</span></span>"),
            "{diff}"
        );
    }

    #[test]
    fn tokens_that_do_not_fit_their_text_leave_it_plain() {
        let text = "é let";
        for tokens in [
            vec![token(3, 99, TokenKind::Keyword)],
            vec![token(1, 3, TokenKind::Keyword)],
            vec![
                token(3, 6, TokenKind::Keyword),
                token(0, 2, TokenKind::Constant),
            ],
            vec![
                token(3, 6, TokenKind::Keyword),
                token(4, 6, TokenKind::Constant),
            ],
        ] {
            let shown = failure_shown(text, tokens.clone());
            assert_eq!(shown, text, "{tokens:?}");
        }
    }

    #[test]
    fn a_note_keeps_its_lines() {
        let mut line = logged(LineOutcome::Failed);
        line.notes.push("Difference:\n-one\n+two".to_owned());
        let page = page_of(line);
        let note = between(&page, "<p class=\"note\">", "</p>");
        assert_eq!(note, "Difference:\n-one\n+two");
        let styles = include_str!("../../templates/page.css");
        assert!(between(styles, ".note {", "}").contains("white-space: pre-wrap"));
    }

    #[test]
    fn images_are_shown_and_other_files_linked_to() {
        let mut line = logged(LineOutcome::Passed);
        for (name, path) in [("reference", "a b/one.png"), ("actual", "a b/one.json")] {
            line.files.push(File {
                name: name.to_owned(),
                path: path.into(),
            });
        }
        let page = page_of(line);
        let images = between(&page, "<div class=\"images\">", "</div>");
        assert!(
            images.contains("src=\"a%20b/one.png\"") && !images.contains("one.json"),
            "{images}"
        );
        let links = between(&page, "<p class=\"links\">", "</p>");
        assert!(
            links.contains("href=\"a%20b/one.json\"") && !links.contains("one.png"),
            "{links}"
        );
    }

    #[test]
    fn a_lines_time_leads_into_the_video() {
        let mut line = logged(LineOutcome::Passed);
        line.seek = Some(1.25);
        let page = page_of(line);
        let link = between(&page, "<a class=\"seek\"", "</a>");
        assert!(
            link.contains("href=\"plays/adds/video.glow.mp4#t=1.250\""),
            "{link}"
        );
        assert!(link.contains("data-at=\"1.250\""), "{link}");
    }

    #[test]
    fn a_check_says_what_became_of_what_it_saw() {
        let says = |acceptance| {
            let mut line = logged(LineOutcome::Passed);
            line.acceptance = acceptance;
            let page = page_of(line);
            (
                page.contains("takes what it saw"),
                page.contains("Taken as the reference"),
            )
        };
        assert_eq!(says(None), (false, false));
        assert_eq!(says(Some(Taking::Offered)), (true, false));
        assert_eq!(says(Some(Taking::Taken)), (false, true));
    }

    #[test]
    fn a_line_is_as_deep_as_the_steps_around_it() {
        let mut line = logged(LineOutcome::Passed);
        line.depth = 3;
        assert!(page_of(line).contains("--depth: 3"));
    }

    #[test]
    fn a_time_is_read_to_the_minute_and_kept_to_the_millisecond() {
        let at: Timestamp = "2024-12-31T23:59:59.500123Z".parse().expect("a time");
        let time = Time::from(at);
        assert_eq!(time.utc, "2024-12-31 23:59 UTC");
        assert_eq!(time.rfc3339, "2024-12-31T23:59:59.500Z");
    }

    #[test]
    fn a_link_spells_out_what_means_something_in_one() {
        let path = Utf8Path::new("a b/50%/c#d?e/č.png");
        assert_eq!(href(path), "a%20b/50%25/c%23d%3Fe/%C4%8D.png");
    }

    #[test]
    fn an_anchor_is_as_readable_as_a_link_allows() {
        assert_eq!(anchor("play::plays::Todo::adds"), "play::plays::Todo::adds");
        assert_eq!(anchor("smoke::Calls back"), "smoke::Calls%20back");
        assert_eq!(anchor("50%#<b>"), "50%25%23%3Cb%3E");
    }

    #[test]
    fn a_duration_is_rounded_to_what_matters() {
        assert_eq!(
            [0, 999, 1_000, 61_950].map(took),
            ["0 ms", "999 ms", "1.0 s", "61.9 s"]
        );
    }

    #[test]
    fn a_scene_is_as_bad_as_its_worst_test() {
        use Verdict::{Failed, Passed, Unfinished};
        assert_eq!([Passed, Unfinished].into_iter().max(), Some(Unfinished));
        assert_eq!([Failed, Unfinished, Passed].into_iter().max(), Some(Failed));
    }
}
