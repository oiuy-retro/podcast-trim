//! The text the user copies into an AI of their choice.
//!
//! Every prompt gives the AI the short audio hash of each episode (the
//! first 12 hex digits of the SHA-256 of the original download) and asks it
//! to start the episode's part of the reply with `hash: <that hash>`. Check
//! cuts refuses a reply whose hash doesn't match, so cuts meant for one
//! episode can't be applied to another.

const RULES: &str = "\
Find every advertisement, sponsor read, promo for another show, and \
listener-support or subscription appeal. Every paid sponsor read or inserted \
ad counts, whatever the product (retail, fragrance, apps, banks, and so on). \
An ad break often holds several ads back to back, so keep going until the \
show itself resumes. Do not mark the show's own content, \
intro music, or outro as an ad unless it contains an ad. Start each range \
at the first line of the ad and end it at the last line of the ad, using the \
timestamps from the transcript.";

pub const TEMPLATE: &str = "\
You are helping me remove advertising from a podcast episode.

Below is a transcript of the episode \"{title}\" from the show \"{show}\". \
The episode is {duration} long. Its audio hash is {hash}. Each line starts \
with [START-END] timestamps in HH:MM:SS.mmm format.

{rules}

Reply with ONLY the following, in exactly this format and nothing else. \
The first line must be exactly:
hash: {hash}
Then the ad ranges, one per line:
HH:MM:SS.mmm-HH:MM:SS.mmm

Example reply, for an episode whose hash is 0123456789ab:
hash: 0123456789ab
00:00:00.000-00:00:42.500
00:23:10.120-00:25:01.900

If there are no ads, reply with the hash line and then the single word NONE \
on the next line. Do not write notes, commands, explanations, or code.

Transcript:
{transcript}";

pub fn build(show: &str, title: &str, duration: &str, hash: &str, transcript: &str) -> String {
    TEMPLATE
        .replace("{rules}", RULES)
        .replace("{show}", show)
        .replace("{title}", title)
        .replace("{duration}", duration)
        .replace("{hash}", hash)
        .replace("{transcript}", transcript)
}

/// One episode in a bulk prompt.
pub struct Item<'a> {
    pub show: &'a str,
    pub title: &'a str,
    pub duration: String,
    pub hash: &'a str,
    pub transcript: &'a str,
}

const BULK_HEAD: &str = "\
You are helping me remove advertising from {count} podcast episodes.

Below are their transcripts. Each episode starts with a header line \
\"hash: <12 hex digits>\", which is that episode's audio hash, then its \
title, show, and length, then its transcript. Each transcript line starts \
with [START-END] timestamps in HH:MM:SS.mmm format, counted from the start \
of that episode. Treat every episode separately.

{rules}

Reply with ONLY the following, in exactly this format and nothing else. \
For each episode, in the same order as below: one line with its hash \
exactly as given (hash: <12 hex digits>), then that episode's ad ranges, \
one per line, as HH:MM:SS.mmm-HH:MM:SS.mmm. If an episode has no ads, put \
the single word NONE under its hash line instead. Give every episode its \
hash line, even with no ads, and never put one episode's ranges under \
another episode's hash.

Example reply, for two episodes whose hashes are 0123456789ab and \
ba9876543210 (use the real hashes given below):
hash: 0123456789ab
00:00:00.000-00:00:42.500
00:23:10.120-00:25:01.900
hash: ba9876543210
NONE

Do not write notes, commands, explanations, or code.
";

/// One combined prompt for several episodes.
pub fn build_bulk(items: &[Item]) -> String {
    let mut out = BULK_HEAD
        .replace("{rules}", RULES)
        .replace("{count}", &items.len().to_string());
    for it in items {
        out.push_str(&format!(
            "\n==========\nhash: {}\nEpisode: \"{}\"\nShow: \"{}\"\nLength: {}\nTranscript:\n{}",
            it.hash, it.title, it.show, it.duration, it.transcript
        ));
        if !it.transcript.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cutlist::parse_sections;

    #[test]
    fn fills() {
        let p = build("S", "T", "01:00:00.000", "0123456789ab", "[00:00:00.000-00:00:01.000] hi\n");
        assert!(p.contains("\"T\" from the show \"S\""));
        assert!(p.ends_with("hi\n"));
        assert!(!p.contains('{'));
        assert!(p.contains("several ads back to back"));
        assert!(p.contains("Do not write notes"));
        assert!(p.contains("The first line must be exactly:\nhash: 0123456789ab\n"));
        // The example reply parses, but with a placeholder hash, so an AI
        // that copies it is caught by Check cuts.
        let ex = p.split("hash is 0123456789ab:\n").nth(1).unwrap().split("\n\n").next().unwrap();
        assert_eq!(parse_sections(ex).unwrap()[0].hash, "0123456789ab");
        let p2 = build("S", "T", "01:00:00.000", "ffffffffffff", "");
        assert!(p2.contains("exactly:\nhash: ffffffffffff\n") && p2.matches("ffffffffffff").count() == 2);
    }

    #[test]
    fn bulk() {
        let items = [
            Item { show: "S", title: "One", duration: "00:10:00.000".into(), hash: "aaaaaaaaaaaa", transcript: "[00:00:00.000-00:00:01.000] one\n" },
            Item { show: "S", title: "Two", duration: "00:20:00.000".into(), hash: "bbbbbbbbbbbb", transcript: "[00:00:00.000-00:00:01.000] two" },
        ];
        let p = build_bulk(&items);
        assert!(p.starts_with("You are helping me remove advertising from 2 podcast episodes."));
        assert!(!p.contains('{'));
        assert!(p.contains("\n==========\nhash: aaaaaaaaaaaa\nEpisode: \"One\"\n"));
        assert!(p.contains("\n==========\nhash: bbbbbbbbbbbb\nEpisode: \"Two\"\n"));
        assert!(p.ends_with("two\n"));
        let ex = p.split("given below):\n").nth(1).unwrap().split("Do not").next().unwrap();
        let s = parse_sections(ex).unwrap();
        assert_eq!((s[0].hash.as_str(), s[0].cuts.len()), ("0123456789ab", 2));
        assert_eq!((s[1].hash.as_str(), s[1].cuts.len()), ("ba9876543210", 0));
        assert_eq!(p.matches("aaaaaaaaaaaa").count(), 1);
    }
}
