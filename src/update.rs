//! The `--check-updates` path: fetch the upstream CHANGELOG and report what has landed since this
//! build's version.

/// Print whether a newer `gallery` release is out, plus the CHANGELOG entries since this build's version.
/// A dev-tool convenience (`cargo run -- --check-updates` / `just update`): fetch the upstream CHANGELOG
/// and compare its released `## [x.y.z]` sections against this crate's own version.
pub(crate) fn check_updates() {
    let current = env!("CARGO_PKG_VERSION");
    let repo = env!("CARGO_PKG_REPOSITORY");
    let Some(url) = raw_changelog_url(repo) else {
        eprintln!("gallery: can't derive a CHANGELOG URL from repository `{repo}`");
        return;
    };
    let changelog = match fetch(&url) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("gallery: couldn't fetch the upstream CHANGELOG ({url}): {e}");
            return;
        }
    };

    let installed = semver::Version::parse(current).ok();
    let mut newer: Vec<(semver::Version, String)> = released_sections(&changelog)
        .into_iter()
        .filter(|(version, _)| installed.as_ref().is_none_or(|cur| version > cur))
        .collect();
    newer.sort_by(|a, b| b.0.cmp(&a.0));

    if newer.is_empty() {
        println!("gallery {current} is up to date — no newer release upstream.");
        return;
    }
    println!("A newer gallery is out (you're on {current}):\n");
    for (version, notes) in newer {
        println!("## {version}\n{}\n", notes.trim());
    }
}

/// `https://github.com/owner/repo(.git)` → the raw CHANGELOG on the `main` branch; `None` for a
/// non-GitHub repository.
fn raw_changelog_url(repo: &str) -> Option<String> {
    let path = repo.strip_prefix("https://github.com/")?;
    let path = path.strip_suffix('/').unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    Some(format!(
        "https://raw.githubusercontent.com/{path}/main/CHANGELOG.md"
    ))
}

/// Fetch `url` over HTTPS in-process (`ureq`) — portable, with no reliance on a system `curl`.
fn fetch(url: &str) -> Result<String, String> {
    ureq::get(url)
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())
}

/// The released `## [x.y.z]` sections of a Keep-a-Changelog document, as `(version, notes)` — skipping
/// `## [Unreleased]` and any heading whose bracketed name isn't a semver version.
fn released_sections(changelog: &str) -> Vec<(semver::Version, String)> {
    let mut sections = Vec::new();
    let mut current: Option<(semver::Version, String)> = None;
    for line in changelog.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if let Some(section) = current.take() {
                sections.push(section);
            }
            // `[x.y.z] - 2026-01-01` → `x.y.z`
            let name = heading
                .trim()
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or_default()
                .trim();
            if let Ok(version) = semver::Version::parse(name) {
                current = Some((version, String::new()));
            }
        } else if let Some((_, notes)) = current.as_mut() {
            notes.push_str(line);
            notes.push('\n');
        }
    }
    if let Some(section) = current.take() {
        sections.push(section);
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_changelog_url_maps_github_to_raw_and_rejects_others() {
        let expected = "https://raw.githubusercontent.com/kubijo/rs-gallery/main/CHANGELOG.md";
        assert_eq!(
            raw_changelog_url("https://github.com/kubijo/rs-gallery.git").unwrap(),
            expected
        );
        assert_eq!(
            raw_changelog_url("https://github.com/kubijo/rs-gallery").unwrap(),
            expected
        );
        assert!(raw_changelog_url("https://gitlab.com/x/y").is_none());
    }

    #[test]
    fn released_sections_skips_unreleased_and_keeps_versioned_notes() {
        let changelog = "\
## [Unreleased]

- wip

## [0.2.0] - 2026-01-02

- added b

## [0.1.0] - 2026-01-01

- added a
";
        let sections = released_sections(changelog);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, semver::Version::new(0, 2, 0));
        assert!(sections[0].1.contains("added b"));
        assert_eq!(sections[1].0, semver::Version::new(0, 1, 0));
        assert!(sections[1].1.contains("added a"));
    }
}
