//! Reading a show's source RSS feed and writing the cleaned per-show feed.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceItem {
    pub guid: String,
    pub title: String,
    pub url: String,
    pub pub_date: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceFeed {
    pub title: String,
    pub items: Vec<SourceItem>,
}

fn decode(s: &str) -> String {
    let s = s.trim();
    if let Some(raw) = s
        .strip_prefix("<![CDATA[")
        .and_then(|x| x.strip_suffix("]]>"))
    {
        return raw.trim().to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(j) = rest.find(';').filter(|j| *j <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..j];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") => u32::from_str_radix(&ent[2..], 16)
                .ok()
                .and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&rest[..=j]),
        }
        rest = &rest[j + 1..];
    }
    out.push_str(rest);
    out
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Text inside the first `<tag ...>...</tag>` in `xml`.
fn element<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let mut from = 0;
    loop {
        let i = from + xml[from..].find(&format!("<{tag}"))?;
        let after = &xml[i + tag.len() + 1..];
        let next = after.chars().next()?;
        if next == '>' || next.is_whitespace() {
            let open_end = i + tag.len() + 1 + after.find('>')?;
            if xml[..open_end].ends_with('/') {
                return Some("");
            }
            let close = xml[open_end..].find(&format!("</{tag}>"))?;
            return Some(&xml[open_end + 1..open_end + close]);
        }
        from = i + 1;
    }
}

fn attr(tag_text: &str, name: &str) -> Option<String> {
    for q in ['"', '\''] {
        let pat = format!("{name}={q}");
        let mut from = 0;
        while let Some(i) = tag_text[from..].find(&pat) {
            let i = from + i;
            let ok = i == 0 || tag_text[..i].ends_with(char::is_whitespace);
            let start = i + pat.len();
            if ok {
                let end = tag_text[start..].find(q)?;
                return Some(decode(&tag_text[start..start + end]));
            }
            from = start;
        }
    }
    None
}

pub fn parse(xml: &str) -> Result<SourceFeed, String> {
    let channel = element(xml, "channel").ok_or("not an RSS feed (no <channel>)")?;
    let head = &channel[..channel.find("<item").unwrap_or(channel.len())];
    let title = element(head, "title").map(decode).unwrap_or_default();
    let mut items = Vec::new();
    let mut rest = channel;
    while let Some(i) = rest.find("<item") {
        let after = &rest[i..];
        let Some(end) = after.find("</item>") else {
            break;
        };
        let item = &after[..end];
        rest = &after[end + 7..];
        let Some(e) = item.find("<enclosure") else {
            continue;
        };
        let etag = &item[e..e + item[e..].find('>').unwrap_or(item.len() - e)];
        let Some(url) = attr(etag, "url") else {
            continue;
        };
        let guid = element(item, "guid")
            .map(decode)
            .filter(|g| !g.is_empty())
            .unwrap_or_else(|| url.clone());
        items.push(SourceItem {
            guid,
            title: element(item, "title").map(decode).unwrap_or_default(),
            url,
            pub_date: element(item, "pubDate").map(decode).unwrap_or_default(),
        });
    }
    Ok(SourceFeed { title, items })
}

#[derive(Clone, Debug)]
pub struct OutItem {
    pub guid: String,
    pub title: String,
    pub pub_date: String,
    pub url: String,
    pub length: u64,
    pub description: String,
}

pub fn generate(show_title: &str, link: &str, items: &[OutItem]) -> String {
    let mut x = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\">\n<channel>\n",
    );
    x.push_str(&format!(
        "<title>{} (trimmed)</title>\n",
        escape(show_title)
    ));
    x.push_str(&format!("<link>{}</link>\n", escape(link)));
    x.push_str("<description>Ad-trimmed episodes served by Podcast Trim.</description>\n");
    for it in items {
        x.push_str("<item>\n");
        x.push_str(&format!("<title>{}</title>\n", escape(&it.title)));
        x.push_str(&format!(
            "<guid isPermaLink=\"false\">{}</guid>\n",
            escape(&it.guid)
        ));
        if !it.pub_date.is_empty() {
            x.push_str(&format!("<pubDate>{}</pubDate>\n", escape(&it.pub_date)));
        }
        x.push_str(&format!(
            "<description>{}</description>\n",
            escape(&it.description)
        ));
        x.push_str(&format!(
            "<enclosure url=\"{}\" length=\"{}\" type=\"audio/mpeg\"/>\n",
            escape(&it.url),
            it.length
        ));
        x.push_str("</item>\n");
    }
    x.push_str("</channel>\n</rss>\n");
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0"?><rss xmlns:itunes="x"><channel>
<title>My &amp; Show</title><itunes:image href="i"/>
<item><title><![CDATA[Ep 2: <Part> two]]></title><guid isPermaLink="false">g2</guid>
<pubDate>Tue, 27 Oct 2026 10:00:00 GMT</pubDate>
<enclosure length="1" type="audio/mpeg" url="https://cdn.example/2.mp3?a=1&amp;b=2"/></item>
<item><title>No audio</title></item>
<item><title>Ep 1</title><enclosure url='https://cdn.example/1.mp3' /></item>
</channel></rss>"#;

    #[test]
    fn parses() {
        let f = parse(FEED).unwrap();
        assert_eq!(f.title, "My & Show");
        assert_eq!(f.items.len(), 2);
        assert_eq!(f.items[0].title, "Ep 2: <Part> two");
        assert_eq!(f.items[0].url, "https://cdn.example/2.mp3?a=1&b=2");
        assert_eq!(f.items[0].guid, "g2");
        assert_eq!(f.items[1].guid, "https://cdn.example/1.mp3");
        assert!(parse("<html></html>").is_err());
        assert_eq!(decode("&#65;&#x42;&bogus; & x"), "AB&bogus; & x");
    }

    #[test]
    fn generates_and_roundtrips() {
        let xml = generate(
            "A <b> & \"c\"",
            "http://h/",
            &[OutItem {
                guid: "g".into(),
                title: "T & U".into(),
                pub_date: "d".into(),
                url: "http://h/media/1/2.mp3?token=abc&x=1".into(),
                length: 99,
                description: "Removed 1:00".into(),
            }],
        );
        assert!(xml.contains("<title>A &lt;b&gt; &amp; &quot;c&quot; (trimmed)</title>"));
        assert!(xml.contains("length=\"99\" type=\"audio/mpeg\""));
        let back = parse(&xml).unwrap();
        assert_eq!(back.items[0].url, "http://h/media/1/2.mp3?token=abc&x=1");
        assert_eq!(back.items[0].title, "T & U");
    }
}
