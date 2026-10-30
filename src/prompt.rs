//! The text the user copies into an AI of their choice.

pub const TEMPLATE: &str = "\
You are helping me remove advertising from a podcast episode.

Below is a transcript of the episode \"{title}\" from the show \"{show}\". \
The episode is {duration} long. Each line starts with [START-END] timestamps \
in HH:MM:SS.mmm format.

Find every advertisement, sponsor read, promo for another show, and \
listener-support or subscription appeal. Do not mark the show's own content, \
intro music, or outro as an ad unless it contains an ad. Start each range \
at the first line of the ad and end it at the last line of the ad, using the \
timestamps from the transcript.

Reply with ONLY the ad ranges, one per line, in exactly this format and nothing else:
HH:MM:SS.mmm-HH:MM:SS.mmm

Example reply:
00:00:00.000-00:00:42.500
00:23:10.120-00:25:01.900

If there are no ads, reply with the single word NONE. Do not write commands, \
explanations, or code.

Transcript:
{transcript}";

pub fn build(show: &str, title: &str, duration: &str, transcript: &str) -> String {
    TEMPLATE
        .replace("{show}", show)
        .replace("{title}", title)
        .replace("{duration}", duration)
        .replace("{transcript}", transcript)
}

#[cfg(test)]
mod tests {
    #[test]
    fn fills() {
        let p = super::build("S", "T", "01:00:00.000", "[00:00:00.000-00:00:01.000] hi\n");
        assert!(p.contains("\"T\" from the show \"S\""));
        assert!(p.ends_with("hi\n"));
        assert!(!p.contains('{'));
    }
}
