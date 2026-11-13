//! "Check for new": fetch every show's feed and merge new episodes, keeping
//! going when one show fails, and summarise the result in one status line.
use crate::rss::{self, SourceFeed};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// What happened to one show.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub title: String,
    /// New episodes added, or why the show couldn't be checked.
    pub result: Result<usize, String>,
}

/// A show to check: (id, title, feed URL).
pub type Target = (u32, String, String);

/// Fetch and merge every target with up to `workers` feeds in flight.
///
/// - `fetch` downloads a URL.
/// - `apply` merges a parsed feed into show `id` and returns how many
///   episodes were new, or `None` if the show was removed meanwhile.
/// - `stop` is checked before each show starts (Cancel).
/// - `done(finished, total)` is called after each show.
///
/// Results come back in the order of `targets`; shows never started because
/// of `stop` are left out.
pub fn check_all(
    targets: &[Target],
    workers: usize,
    fetch: &(dyn Fn(&str) -> Result<Vec<u8>, String> + Sync),
    apply: &(dyn Fn(u32, &SourceFeed) -> Option<usize> + Sync),
    stop: &(dyn Fn() -> bool + Sync),
    done: &(dyn Fn(usize, usize) + Sync),
) -> Vec<Outcome> {
    let next = AtomicUsize::new(0);
    let finished = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<Outcome>>> = Mutex::new(vec![None; targets.len()]);
    let one = |(id, title, url): &Target| -> Outcome {
        let result = fetch(url)
            .and_then(|b| rss::parse(&String::from_utf8_lossy(&b)))
            .and_then(|feed| apply(*id, &feed).ok_or_else(|| "removed while checking".to_string()));
        Outcome {
            title: title.clone(),
            result,
        }
    };
    std::thread::scope(|s| {
        for _ in 0..workers.clamp(1, targets.len().max(1)) {
            s.spawn(|| loop {
                if stop() {
                    break;
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(t) = targets.get(i) else { break };
                let o = one(t);
                slots.lock().unwrap()[i] = Some(o);
                done(finished.fetch_add(1, Ordering::SeqCst) + 1, targets.len());
            });
        }
    });
    slots.into_inner().unwrap().into_iter().flatten().collect()
}

fn plural(n: usize, word: &str) -> String {
    crate::store::plural(n, word)
}

/// Keep error text short enough for the status line.
fn short(e: &str) -> String {
    let e = e.trim().trim_end_matches('.');
    let line = e.lines().next().unwrap_or("");
    if line.chars().count() > 60 {
        format!("{}...", line.chars().take(57).collect::<String>())
    } else {
        line.to_string()
    }
}

/// Up to this many shows are listed one by one; more get a compact total.
pub const LIST_MAX: usize = 3;

/// One status line for the whole check.
pub fn summary(outcomes: &[Outcome], total: usize, cancelled: bool) -> String {
    if total == 0 {
        return "No shows to check. Add a show first.".into();
    }
    let mut out = if total <= LIST_MAX && !cancelled {
        outcomes
            .iter()
            .map(|o| match &o.result {
                Ok(0) => format!("{}: no new episodes.", o.title),
                Ok(n) => format!("{}: {}.", o.title, plural(*n, "new episode")),
                Err(e) => format!("{}: couldn't check ({}).", o.title, short(e)),
            })
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        let new: Vec<_> = outcomes
            .iter()
            .filter_map(|o| match o.result {
                Ok(n) if n > 0 => Some((o.title.as_str(), n)),
                _ => None,
            })
            .collect();
        let failed: Vec<_> = outcomes.iter().filter(|o| o.result.is_err()).collect();
        let sum: usize = new.iter().map(|(_, n)| n).sum();
        let mut s = format!("Checked {}: ", plural(outcomes.len(), "show"));
        if sum == 0 {
            s.push_str("no new episodes.");
        } else {
            let each = new
                .iter()
                .map(|(t, n)| format!("{t} {n}"))
                .collect::<Vec<_>>()
                .join(", ");
            s.push_str(&format!("{} ({each}).", plural(sum, "new episode")));
        }
        if !failed.is_empty() {
            let names = failed
                .iter()
                .map(|o| format!("{} ({})", o.title, short(o.result.as_ref().err().unwrap())))
                .collect::<Vec<_>>()
                .join(", ");
            s.push_str(&format!(" Couldn't check {names}."));
        }
        s
    };
    if cancelled {
        out.push_str(&format!(
            " Cancelled before {} were checked.",
            plural(total - outcomes.len(), "show")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Library;

    fn feed(title: &str, guids: &[&str]) -> Vec<u8> {
        let items: String = guids
            .iter()
            .map(|g| {
                format!(
                    "<item><title>{g}</title><guid>{g}</guid>\
                     <enclosure url=\"https://x/{g}.mp3\" type=\"audio/mpeg\"/></item>"
                )
            })
            .collect();
        format!("<rss><channel><title>{title}</title>{items}</channel></rss>").into_bytes()
    }

    fn run(lib: &Mutex<Library>, workers: usize, stop_after: Option<usize>) -> (Vec<Outcome>, usize) {
        let targets: Vec<Target> = lib
            .lock()
            .unwrap()
            .shows
            .iter()
            .map(|s| (s.id, s.title.clone(), s.feed_url.clone()))
            .collect();
        let fetch = |url: &str| -> Result<Vec<u8>, String> {
            match url {
                "https://a/feed" => Ok(feed("A", &["a1", "a2", "a3"])),
                "https://b/feed" => Ok(feed("B", &["b1"])),
                "https://bad/feed" => Err("HTTP 404".into()),
                _ => Ok(b"<html>not a feed</html>".to_vec()),
            }
        };
        let apply = |id: u32, f: &SourceFeed| {
            let mut l = lib.lock().unwrap();
            l.show_mut(id).map(|s| crate::pipeline::merge_feed(s, f))
        };
        let count = AtomicUsize::new(0);
        let stop = || stop_after.is_some_and(|n| count.load(Ordering::SeqCst) >= n);
        let done = |_: usize, _: usize| {
            count.fetch_add(1, Ordering::SeqCst);
        };
        let n = targets.len();
        (check_all(&targets, workers, &fetch, &apply, &stop, &done), n)
    }

    fn library() -> Mutex<Library> {
        let mut l = Library::default();
        l.add_show("A".into(), "https://a/feed".into());
        l.add_show("Bad".into(), "https://bad/feed".into());
        l.add_show("B".into(), "https://b/feed".into());
        l.add_show("Html".into(), "https://html/feed".into());
        // A already has one episode, so two are new.
        let a = l.show_mut(1).unwrap();
        crate::pipeline::merge_feed(a, &rss::parse(&String::from_utf8_lossy(&feed("A", &["a1"]))).unwrap());
        Mutex::new(l)
    }

    #[test]
    fn checks_every_show_and_keeps_going_after_errors() {
        for workers in [1, 3] {
            let lib = library();
            let (out, n) = run(&lib, workers, None);
            assert_eq!(n, 4);
            let got: Vec<_> = out.iter().map(|o| (o.title.as_str(), o.result.clone())).collect();
            assert_eq!(got[0], ("A", Ok(2)));
            assert_eq!(got[1], ("Bad", Err("HTTP 404".into())));
            assert_eq!(got[2], ("B", Ok(1)));
            assert!(got[3].1.is_err());
            let l = lib.lock().unwrap();
            assert_eq!(l.show(1).unwrap().episodes.len(), 3);
            assert_eq!(l.show(3).unwrap().episodes.len(), 1);
            drop(l);
            // A second check finds nothing new.
            let (again, _) = run(&lib, workers, None);
            assert_eq!(again[0].result, Ok(0));
            assert_eq!(again[2].result, Ok(0));
        }
    }

    #[test]
    fn stop_leaves_the_rest_unchecked() {
        let lib = library();
        let (out, n) = run(&lib, 1, Some(2));
        assert_eq!(out.len(), 2);
        assert!(summary(&out, n, true).ends_with("Cancelled before 2 shows were checked."));
    }

    #[test]
    fn summary_lists_a_few_shows() {
        let o = vec![
            Outcome { title: "Lateral".into(), result: Ok(2) },
            Outcome { title: "Show B".into(), result: Ok(0) },
            Outcome { title: "Show C".into(), result: Err("timed out.".into()) },
        ];
        assert_eq!(
            summary(&o, 3, false),
            "Lateral: 2 new episodes. Show B: no new episodes. Show C: couldn't check (timed out)."
        );
        assert_eq!(summary(&o[..1], 1, false), "Lateral: 2 new episodes.");
        assert_eq!(summary(&[], 0, false), "No shows to check. Add a show first.");
    }

    #[test]
    fn summary_totals_many_shows() {
        let mut o: Vec<_> = (0..5)
            .map(|i| Outcome { title: format!("S{i}"), result: Ok(0) })
            .collect();
        assert_eq!(summary(&o, 5, false), "Checked 5 shows: no new episodes.");
        o[1].result = Ok(3);
        o[4].result = Ok(1);
        o[2].result = Err("x".into());
        assert_eq!(
            summary(&o, 5, false),
            "Checked 5 shows: 4 new episodes (S1 3, S4 1). Couldn't check S2 (x)."
        );
    }

    #[test]
    fn long_errors_are_shortened() {
        let e = "a".repeat(100);
        let o = vec![Outcome { title: "T".into(), result: Err(e) }];
        let s = summary(&o, 1, false);
        assert!(s.len() < 90, "{s}");
        assert!(s.contains("...)"));
    }
}
