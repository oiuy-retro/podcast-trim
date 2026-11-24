//! Which episodes count as NEW.
//!
//! A show's episode list is kept newest first (see `pipeline::merge_feed`).
//! When a show is added, only its newest `n` episodes stay NEW; every older
//! episode that has not been downloaded is marked seen and listed as OLD.
//! Episodes that appear later, when the feed is checked again, are NEW.
//! Shows added before this rule existed get it applied once, the next time
//! the library loads. Only the `seen` flag changes; nothing else is touched.
//!
//! An episode that has been sent (its cleaned file was fetched in full by
//! the podcast app, state `Delivered`) also counts as seen: it is marked
//! seen when it is sent, and libraries from before this rule get it once.
//! It keeps its SENT tag and is drawn dimmed like OLD. Mark as unseen on a
//! SENT episode draws it at full brightness again.

use crate::store::{Episode, Library, Show, State};

/// How the episode list labels an episode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// Not downloaded and not seen.
    New,
    /// Not downloaded, but marked as already seen elsewhere.
    Old,
    /// Sent to the podcast app and seen (the usual case for a sent episode).
    Sent,
    /// Downloaded or further along and not sent, or sent and marked unseen.
    Other,
}

pub fn mark(ep: &Episode) -> Mark {
    match (ep.state, ep.seen) {
        (State::New, false) => Mark::New,
        (State::New, true) => Mark::Old,
        (State::Delivered, true) => Mark::Sent,
        _ => Mark::Other,
    }
}

/// Drawn with the faded look: OLD, and sent episodes that count as seen.
pub fn dimmed(ep: &Episode) -> bool {
    matches!(mark(ep), Mark::Old | Mark::Sent)
}

/// One-time upgrade: every episode already sent counts as seen. Runs once
/// per library, so a sent episode later marked unseen stays unseen.
/// Returns how many episodes were marked.
pub fn migrate_sent(lib: &mut Library) -> usize {
    if lib.sent_seen_done {
        return 0;
    }
    lib.sent_seen_done = true;
    let mut n = 0;
    for e in lib.shows.iter_mut().flat_map(|s| s.episodes.iter_mut()) {
        if e.state == State::Delivered && !e.seen {
            e.seen = true;
            n += 1;
        }
    }
    n
}

/// Keep the newest `keep_new` episodes as they are and mark every older
/// not-yet-downloaded episode as seen. Records that the rule has run for
/// this show. Returns how many episodes were marked.
pub fn apply_rule(show: &mut Show, keep_new: usize) -> usize {
    let mut n = 0;
    for e in show.episodes.iter_mut().skip(keep_new) {
        if e.state == State::New && !e.seen {
            e.seen = true;
            n += 1;
        }
    }
    show.seen_rule_done = true;
    n
}

/// One-time upgrade for libraries written before 0.1.2: apply the rule to
/// every show that has not had it. Returns (shows changed, episodes marked).
pub fn migrate(lib: &mut Library, keep_new: usize) -> (usize, usize) {
    let (mut shows, mut eps) = (0, 0);
    for s in lib.shows.iter_mut().filter(|s| !s.seen_rule_done) {
        let n = apply_rule(s, keep_new);
        if n > 0 {
            shows += 1;
            eps += n;
        }
    }
    (shows, eps)
}

/// Set `seen` on every episode listed below (older than) `guid`.
/// Returns how many changed.
pub fn set_older(show: &mut Show, guid: &str, seen: bool) -> usize {
    let Some(at) = show.episodes.iter().position(|e| e.guid == guid) else {
        return 0;
    };
    let mut n = 0;
    for e in show.episodes[at + 1..].iter_mut().filter(|e| e.seen != seen) {
        e.seen = seen;
        n += 1;
    }
    n
}

/// Set `seen` on the given episodes. Returns how many changed.
pub fn set(show: &mut Show, guids: &[String], seen: bool) -> usize {
    let mut n = 0;
    for e in show.episodes.iter_mut() {
        if e.seen != seen && guids.contains(&e.guid) {
            e.seen = seen;
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Library;

    fn show_with(lib: &mut Library, states: &[State]) -> u32 {
        let id = lib.add_show("S".into(), "u".into());
        let s = lib.show_mut(id).unwrap();
        for (i, st) in states.iter().enumerate() {
            s.episodes.push(Episode {
                guid: format!("g{i}"),
                title: format!("E{i}"),
                pub_date: String::new(),
                source_url: String::new(),
                state: *st,
                record: Default::default(),
                cleaned_bytes: 0,
                served: 0,
                seen: false,
                image_url: String::new(),
            });
        }
        id
    }

    fn marks(s: &Show) -> Vec<Mark> {
        s.episodes.iter().map(mark).collect()
    }

    #[test]
    fn only_newest_are_new() {
        let mut lib = Library::default();
        let id = show_with(&mut lib, &[State::New; 4]);
        let s = lib.show_mut(id).unwrap();
        assert_eq!(apply_rule(s, 1), 3);
        assert!(s.seen_rule_done);
        assert_eq!(marks(s), [Mark::New, Mark::Old, Mark::Old, Mark::Old]);
        let id = show_with(&mut lib, &[State::New; 4]);
        let s = lib.show_mut(id).unwrap();
        assert_eq!(apply_rule(s, 0), 4);
        let id = show_with(&mut lib, &[State::New; 2]);
        assert_eq!(apply_rule(lib.show_mut(id).unwrap(), 5), 0);
    }

    #[test]
    fn processed_episodes_are_not_old() {
        let mut lib = Library::default();
        let id = show_with(&mut lib, &[State::New, State::Ready, State::Transcribed, State::New]);
        let s = lib.show_mut(id).unwrap();
        assert_eq!(apply_rule(s, 1), 1);
        assert_eq!(marks(s), [Mark::New, Mark::Other, Mark::Other, Mark::Old]);
    }

    #[test]
    fn migration_runs_once_and_keeps_data() {
        // A library.json from 0.1.1: no `seen` or `seen_rule_done` fields.
        let old = r#"{"next_id":2,"shows":[{"id":1,"title":"Lateral","feed_url":"u",
            "token":"t","episodes":[
            {"guid":"a","title":"A","pub_date":"d1","source_url":"x","state":"New"},
            {"guid":"b","title":"B","pub_date":"d2","source_url":"y","state":"Ready",
             "record":{"sha256":"ab","source_bytes":5,"duration":9.0,"transcript":"t",
             "cuts":[{"start":1.0,"end":2.0}],"ffmpeg_args":["-i"]},"cleaned_bytes":7,"served":1},
            {"guid":"c","title":"C","pub_date":"d3","source_url":"z","state":"New"},
            {"guid":"d","title":"D","pub_date":"d4","source_url":"w","state":"New"}]}]}"#;
        let mut lib: Library = serde_json::from_str(old).unwrap();
        let before = serde_json::to_value(&lib).unwrap();
        assert!(lib.shows[0].episodes.iter().all(|e| !e.seen));
        assert_eq!(migrate(&mut lib, 1), (1, 2));
        assert_eq!(
            marks(&lib.shows[0]),
            [Mark::New, Mark::Other, Mark::Old, Mark::Old]
        );
        // Second load: nothing more happens, even after marking unseen.
        set(&mut lib.shows[0], &["c".into()], false);
        assert_eq!(migrate(&mut lib, 1), (0, 0));
        // Everything except the new flags is unchanged.
        let mut after = serde_json::to_value(&lib).unwrap();
        let show = after["shows"][0].as_object_mut().unwrap();
        assert_eq!(show.remove("seen_rule_done"), Some(true.into()));
        for e in show["episodes"].as_array_mut().unwrap() {
            e.as_object_mut().unwrap().remove("seen");
        }
        let mut before = before;
        let bs = before["shows"][0].as_object_mut().unwrap();
        bs.remove("seen_rule_done");
        for e in bs["episodes"].as_array_mut().unwrap() {
            e.as_object_mut().unwrap().remove("seen");
        }
        assert_eq!(before, after);
        // And it still round-trips.
        let text = serde_json::to_string(&lib).unwrap();
        let back: Library = serde_json::from_str(&text).unwrap();
        assert_eq!(marks(&back.shows[0]), marks(&lib.shows[0]));
    }

    #[test]
    fn new_shows_are_not_migrated_twice() {
        let mut lib = Library::default();
        let id = show_with(&mut lib, &[State::New; 3]);
        apply_rule(lib.show_mut(id).unwrap(), 1);
        // Episodes found later by Check for new are NEW and stay NEW.
        let s = lib.show_mut(id).unwrap();
        s.episodes.insert(0, s.episodes[2].clone());
        s.episodes[0].guid = "fresh".into();
        s.episodes[0].seen = false;
        assert_eq!(migrate(&mut lib, 1), (0, 0));
        assert_eq!(mark(&lib.show(id).unwrap().episodes[0]), Mark::New);
    }

    #[test]
    fn sent_episodes_are_seen_and_dimmed() {
        // A 0.1.1 library with one ready and one sent episode.
        let old = r#"{"next_id":2,"shows":[{"id":1,"title":"S","feed_url":"u",
            "token":"t","episodes":[
            {"guid":"a","title":"A","pub_date":"d","source_url":"x","state":"Ready"},
            {"guid":"b","title":"B","pub_date":"d","source_url":"y","state":"Delivered","served":1}]}]}"#;
        let mut lib: Library = serde_json::from_str(old).unwrap();
        assert!(!lib.sent_seen_done);
        assert_eq!(marks(&lib.shows[0]), [Mark::Other, Mark::Other]);
        assert_eq!(migrate_sent(&mut lib), 1);
        assert!(lib.sent_seen_done);
        assert_eq!(marks(&lib.shows[0]), [Mark::Other, Mark::Sent]);
        assert!(!dimmed(&lib.shows[0].episodes[0]));
        assert!(dimmed(&lib.shows[0].episodes[1]));
        // Marked unseen by hand: full brightness, and a later load keeps it.
        set(&mut lib.shows[0], &["b".into()], false);
        assert!(!dimmed(&lib.shows[0].episodes[1]));
        let text = serde_json::to_string(&lib).unwrap();
        let mut back: Library = serde_json::from_str(&text).unwrap();
        assert_eq!(migrate_sent(&mut back), 0);
        assert_eq!(marks(&back.shows[0]), [Mark::Other, Mark::Other]);
        // The NEW rule does not touch sent episodes.
        assert_eq!(migrate(&mut back, 0), (0, 0));
    }

    #[test]
    fn mark_older_and_selection() {
        let mut lib = Library::default();
        let id = show_with(&mut lib, &[State::New; 5]);
        let s = lib.show_mut(id).unwrap();
        assert_eq!(set_older(s, "g1", true), 3);
        assert_eq!(
            marks(s),
            [Mark::New, Mark::New, Mark::Old, Mark::Old, Mark::Old]
        );
        assert_eq!(set_older(s, "g1", true), 0);
        assert_eq!(set_older(s, "missing", true), 0);
        assert_eq!(set_older(s, "g4", true), 0);
        assert_eq!(set(s, &["g0".into(), "g3".into()], true), 1);
        assert_eq!(set(s, &["g3".into(), "g4".into()], false), 2);
        assert_eq!(
            marks(s),
            [Mark::Old, Mark::New, Mark::Old, Mark::New, Mark::New]
        );
        assert_eq!(set_older(s, "g0", false), 1);
        assert!(s.episodes[1..].iter().all(|e| !e.seen));
    }
}
