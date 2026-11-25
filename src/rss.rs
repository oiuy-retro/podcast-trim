//! Reading a show's source RSS feed and writing the cleaned per-show feed.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceItem {
    pub guid: String,
    pub title: String,
    pub url: String,
    pub pub_date: String,
    /// Episode artwork URL, empty when the item has none.
    pub image_url: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceFeed {
    pub title: String,
    /// Show artwork URL, empty when the feed has none.
    pub image_url: String,
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

/// Every opening (or self-closing) `<tag ...>` in `xml`, as its text
/// without the closing `>`.
fn open_tags<'a>(xml: &'a str, tag: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    let pat = format!("<{tag}");
    let mut from = 0;
    std::iter::from_fn(move || loop {
        let i = from + xml[from..].find(&pat)?;
        let after = &xml[i + pat.len()..];
        from = i + 1;
        let next = after.chars().next()?;
        if next == '>' || next == '/' || next.is_whitespace() {
            let end = after.find('>').map(|e| i + pat.len() + e).unwrap_or(xml.len());
            return Some(&xml[i..end]);
        }
    })
}

/// `xml` with every `<tag>...</tag>` block taken out.
fn without(xml: &str, tag: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    while let Some(i) = rest.find(&open) {
        out.push_str(&rest[..i]);
        match rest[i..].find(&close) {
            Some(j) => rest = &rest[i + j + close.len()..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Artwork in a channel head or an item: `<itunes:image href>` first, then
/// (items only) `<media:thumbnail url>`, then an image `<media:content url>`.
fn artwork(xml: &str, media: bool) -> String {
    let non_empty = |v: Option<String>| v.filter(|u| !u.trim().is_empty()).map(|u| u.trim().to_string());
    if let Some(u) = open_tags(xml, "itunes:image").find_map(|t| non_empty(attr(t, "href"))) {
        return u;
    }
    if !media {
        return String::new();
    }
    if let Some(u) = open_tags(xml, "media:thumbnail").find_map(|t| non_empty(attr(t, "url"))) {
        return u;
    }
    open_tags(xml, "media:content")
        .filter(|t| {
            attr(t, "medium").is_some_and(|m| m.eq_ignore_ascii_case("image"))
                || attr(t, "type").is_some_and(|m| m.to_ascii_lowercase().starts_with("image/"))
        })
        .find_map(|t| non_empty(attr(t, "url")))
        .unwrap_or_default()
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
    // The RSS 2.0 <image> block has its own <title>; skip it for the show's.
    let title = element(&without(head, "image"), "title").map(decode).unwrap_or_default();
    let mut image_url = artwork(head, false);
    if image_url.is_empty() {
        image_url = element(head, "image")
            .and_then(|i| element(i, "url"))
            .map(decode)
            .unwrap_or_default();
    }
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
            image_url: artwork(item, true),
        });
    }
    Ok(SourceFeed {
        title,
        image_url,
        items,
    })
}

#[derive(Clone, Debug)]
pub struct OutItem {
    pub guid: String,
    pub title: String,
    pub pub_date: String,
    pub url: String,
    pub length: u64,
    pub description: String,
    /// Episode artwork from the source feed; empty for none.
    pub image_url: String,
}

/// The cleaned feed. `show_image` (the source feed's artwork, or empty) is
/// written as both `<itunes:image>` and an RSS 2.0 `<image>`; podcast apps
/// such as AntennaPod read either.
pub fn generate(show_title: &str, link: &str, show_image: &str, items: &[OutItem]) -> String {
    let mut x = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\">\n<channel>\n",
    );
    let title = format!("{} (trimmed)", show_title);
    x.push_str(&format!("<title>{}</title>\n", escape(&title)));
    x.push_str(&format!("<link>{}</link>\n", escape(link)));
    x.push_str("<description>Ad-trimmed episodes served by Podcast Trim.</description>\n");
    if !show_image.is_empty() {
        x.push_str(&format!("<itunes:image href=\"{}\"/>\n", escape(show_image)));
        x.push_str(&format!(
            "<image><url>{}</url><title>{}</title><link>{}</link></image>\n",
            escape(show_image),
            escape(&title),
            escape(link)
        ));
    }
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
        if !it.image_url.is_empty() {
            x.push_str(&format!("<itunes:image href=\"{}\"/>\n", escape(&it.image_url)));
        }
        x.push_str(&format!(
            "<enclosure url=\"{}\" length=\"{}\" type=\"audio/ogg\"/>\n",
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
            "",
            &[OutItem {
                guid: "g".into(),
                title: "T & U".into(),
                pub_date: "d".into(),
                url: "http://h/media/1/2.opus?token=abc&x=1".into(),
                length: 99,
                description: "Removed 1:00".into(),
                image_url: String::new(),
            }],
        );
        assert!(xml.contains("xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\""));
        assert!(!xml.contains("image"));
        assert!(xml.contains("<title>A &lt;b&gt; &amp; &quot;c&quot; (trimmed)</title>"));
        assert!(xml.contains("length=\"99\" type=\"audio/ogg\""));
        let back = parse(&xml).unwrap();
        assert_eq!(back.items[0].url, "http://h/media/1/2.opus?token=abc&x=1");
        assert_eq!(back.items[0].title, "T & U");
    }

    #[test]
    fn channel_artwork() {
        // itunes:image is preferred over <image><url>.
        let f = parse(r#"<rss><channel><title>S</title>
<image><url>http://x/rss.jpg</url><title>Logo</title></image>
<itunes:image href="http://x/it.jpg?a=1&amp;b=2"/>
<item><itunes:image href="http://x/item.jpg"/><enclosure url="a"/></item></channel></rss>"#)
        .unwrap();
        assert_eq!(f.title, "S");
        assert_eq!(f.image_url, "http://x/it.jpg?a=1&b=2");
        // RSS 2.0 <image> only, placed before the title; an item's
        // itunes:image is not the show's.
        let f = parse(r#"<rss><channel><image><url> http://x/rss.jpg?s=1&amp;t=2 </url><title>Logo</title></image>
<title>Show</title><item><itunes:image href="http://x/item.jpg"/><enclosure url="a"/></item></channel></rss>"#)
        .unwrap();
        assert_eq!(f.title, "Show");
        assert_eq!(f.image_url, "http://x/rss.jpg?s=1&t=2");
        assert_eq!(f.items[0].image_url, "http://x/item.jpg");
        // None at all.
        let f = parse("<rss><channel><title>S</title><item><enclosure url='a'/></item></channel></rss>").unwrap();
        assert_eq!(f.image_url, "");
        assert_eq!(f.items[0].image_url, "");
    }

    #[test]
    fn item_artwork() {
        let f = parse(r#"<rss><channel><title>S</title>
<item><guid>1</guid><itunes:image href='http://x/1.jpg?a=1&amp;b=2' /><media:thumbnail url="http://x/t.jpg"/><enclosure url="a1"/></item>
<item><guid>2</guid><media:thumbnail url='http://x/2.jpg'/><enclosure url="a2"/></item>
<item><guid>3</guid><media:content url="http://x/3.mp3" type="audio/mpeg"/><media:content medium="image" url="http://x/3.jpg"/><enclosure url="a3"/></item>
<item><guid>4</guid><media:content url="http://x/4.png" type="Image/PNG"></media:content><enclosure url="a4"/></item>
<item><guid>5</guid><media:content url="http://x/5.mp3" type="audio/mpeg" medium="audio"/><itunes:image href=""/><enclosure url="a5"/></item>
</channel></rss>"#)
        .unwrap();
        let urls: Vec<&str> = f.items.iter().map(|i| i.image_url.as_str()).collect();
        assert_eq!(
            urls,
            ["http://x/1.jpg?a=1&b=2", "http://x/2.jpg", "http://x/3.jpg", "http://x/4.png", ""]
        );
    }

    #[test]
    fn generates_artwork() {
        let it = |img: &str| OutItem {
            guid: "g".into(),
            title: "T".into(),
            pub_date: String::new(),
            url: "u".into(),
            length: 1,
            description: String::new(),
            image_url: img.into(),
        };
        let xml = generate("S & T", "http://h/feed/1.xml?token=a&b", "http://x/s.jpg?a=1&b=2", &[it("http://x/e.jpg?c=3&d=4"), it("")]);
        assert!(xml.contains("<rss version=\"2.0\" xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\">"));
        let channel_head = &xml[..xml.find("<item>").unwrap()];
        assert!(channel_head.contains("<itunes:image href=\"http://x/s.jpg?a=1&amp;b=2\"/>"));
        assert!(channel_head.contains(
            "<image><url>http://x/s.jpg?a=1&amp;b=2</url><title>S &amp; T (trimmed)</title><link>http://h/feed/1.xml?token=a&amp;b</link></image>"
        ));
        let items: Vec<&str> = xml.split("<item>").skip(1).collect();
        assert!(items[0].contains("<itunes:image href=\"http://x/e.jpg?c=3&amp;d=4\"/>"));
        assert!(!items[1].contains("image"));
        // Our own parser reads it back.
        let back = parse(&xml).unwrap();
        assert_eq!(back.image_url, "http://x/s.jpg?a=1&b=2");
        assert_eq!(back.title, "S & T (trimmed)");
        assert_eq!(back.items[0].image_url, "http://x/e.jpg?c=3&d=4");
        assert_eq!(back.items[1].image_url, "");
    }
}
