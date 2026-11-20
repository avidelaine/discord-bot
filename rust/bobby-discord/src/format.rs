//! Discord formatting: splitting text and embeds to Discord's limits (message 2000 chars; embed: title 256,
//! description 4096, 25 fields, field name 256 / value 1024, 6000 in all; 10 embeds per message).
//! Port of bot/format.js. Lengths count chars (JS counts UTF-16 units); we never slice inside a char.

use bobby_proto::msg::Embed;
use serenity::all::{CreateAllowedMentions, CreateEmbed, CreateEmbedFooter, CreateMessage};

pub const COLOR_INFO: u32 = 0x5865f2;
pub const COLOR_TODO: u32 = 0x57f287;
pub const COLOR_RIGHTS: u32 = 0xfee75c;
pub const COLOR_NICKNAMES: u32 = 0xeb459e;
pub const COLOR_CLAUDE: u32 = 0xd97757;

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmbedOut {
    pub title: String,
    pub description: Option<String>,
    pub color: u32,
    pub fields: Vec<Field>,
    pub footer: Option<String>,
}

/// One Discord message: text content or embeds, never both.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Out {
    pub content: Option<String>,
    pub embeds: Vec<EmbedOut>,
}

fn len(s: &str) -> usize {
    s.chars().count()
}

/// Truncates to `n` chars, ending with an ellipsis.
fn cut(s: &str, n: usize) -> String {
    if len(s) > n {
        let mut t: String = s.chars().take(n - 1).collect();
        t.push('…');
        t
    } else {
        s.to_string()
    }
}

/// Splits text at line breaks into pieces of at most `max` chars; a code block cut in two is closed and reopened.
pub fn chunks(text: &str, max: usize) -> Vec<String> {
    let lim = max.saturating_sub(8).max(1); // room to close a code block
    let mut out = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    while !rest.is_empty() {
        let mut n = if rest.len() <= lim {
            rest.len()
        } else {
            // last line break at index <= lim, like lastIndexOf('\n', lim)
            rest[..=lim].iter().rposition(|&c| c == '\n').unwrap_or(0)
        };
        if n == 0 {
            n = lim;
        }
        let mut piece: String = rest[..n].iter().collect();
        rest.drain(..n);
        if rest.first() == Some(&'\n') {
            rest.remove(0);
        }
        if piece.matches("```").count() % 2 == 1 {
            piece.push_str("\n```");
            if !rest.is_empty() {
                // never reopen an empty rest
                let mut r: Vec<char> = "```\n".chars().collect();
                r.append(&mut rest);
                rest = r;
            }
        }
        out.push(piece);
    }
    out
}

fn size(e: &EmbedOut) -> usize {
    len(&e.title)
        + e.description.as_deref().map_or(0, len)
        + e.footer.as_deref().map_or(0, len)
        + e.fields.iter().map(|f| len(&f.name) + len(&f.value)).sum::<usize>()
}

/// One logical embed -> as many embeds as Discord's limits need. Long field values continue in extra fields.
pub fn embed(title: &str, color: u32, description: Option<&str>, fields: &[Field], footer: Option<&str>) -> Vec<EmbedOut> {
    let suite = || cut(&format!("{title} (suite)"), 256);
    let mut flat = Vec::new();
    for f in fields {
        let value = if f.value.is_empty() { "-" } else { f.value.as_str() };
        for (i, v) in chunks(value, 1024).into_iter().enumerate() {
            let name = if i > 0 { format!("{} (suite)", f.name) } else { f.name.clone() };
            flat.push(Field { name: cut(&name, 256), value: v, inline: f.inline });
        }
    }
    let mut descs = description.filter(|d| !d.is_empty()).map(|d| chunks(d, 4096)).unwrap_or_default().into_iter();
    let mut out = vec![EmbedOut { title: cut(title, 256), description: descs.next(), color, fields: vec![], footer: None }];
    for d in descs {
        out.push(EmbedOut { title: suite(), description: Some(d), color, fields: vec![], footer: None });
    }
    for f in flat {
        let full = {
            let cur = out.last().unwrap();
            cur.fields.len() >= 25 || size(cur) + len(&f.name) + len(&f.value) > 5500
        };
        if full {
            out.push(EmbedOut { title: suite(), description: None, color, fields: vec![], footer: None });
        }
        out.last_mut().unwrap().fields.push(f);
    }
    if let Some(ft) = footer {
        out.last_mut().unwrap().footer = Some(cut(ft, 400)); // fields stop at 5500: the footer stays under 6000
    }
    out
}

/// What a `Post` sends: text (content chunks of 2000) then embeds (split, grouped 10 and 6000 chars per message).
pub fn messages(text: Option<&str>, embeds: &[Embed]) -> Vec<Out> {
    let mut out: Vec<Out> = text
        .filter(|t| !t.trim().is_empty()) // Discord refuses a blank message
        .map(|t| chunks(t, 2000))
        .unwrap_or_default()
        .into_iter()
        .map(|c| Out { content: Some(c), embeds: vec![] })
        .collect();
    let first_embed = out.len(); // embeds never join a text message
    for e in embeds {
        let desc = (!e.description.is_empty()).then_some(e.description.as_str());
        let fields: Vec<Field> =
            e.fields.iter().map(|f| Field { name: f.name.clone(), value: f.value.clone(), inline: f.inline }).collect();
        for eo in embed(&e.title, e.color.unwrap_or(COLOR_INFO), desc, &fields, e.footer.as_deref()) {
            let fits = out.len() > first_embed
                && out.last().is_some_and(|m| {
                    m.embeds.len() < 10 && m.embeds.iter().map(size).sum::<usize>() + size(&eo) <= 6000
                });
            if fits {
                out.last_mut().unwrap().embeds.push(eo);
            } else {
                out.push(Out { content: None, embeds: vec![eo] });
            }
        }
    }
    out
}

/// Serenity embeds of one message.
pub fn to_embeds(o: &Out) -> Vec<CreateEmbed> {
    o.embeds
        .iter()
        .map(|e| {
            let mut ce = CreateEmbed::new().title(&e.title).color(e.color);
            if let Some(d) = &e.description {
                ce = ce.description(d);
            }
            ce = ce.fields(e.fields.iter().map(|f| (f.name.clone(), f.value.clone(), f.inline)));
            if let Some(ft) = &e.footer {
                ce = ce.footer(CreateEmbedFooter::new(ft));
            }
            ce
        })
        .collect()
}

/// Message payload that never pings: a YouTube title or nickname like "@everyone" stays inert.
pub fn to_serenity(o: &Out) -> CreateMessage {
    let mut m = CreateMessage::new().allowed_mentions(CreateAllowedMentions::new()); // empty = parse nothing
    if let Some(c) = &o.content {
        m = m.content(c);
    }
    let embeds = to_embeds(o);
    if embeds.is_empty() {
        m
    } else {
        m.embeds(embeds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fences(s: &str) -> usize {
        s.matches("```").count()
    }
    fn long() -> String {
        (0..300).map(|i| format!("ligne {i} {}", "x".repeat(20))).collect::<Vec<_>>().join("\n")
    }
    fn small(n: usize) -> Vec<Embed> {
        (0..n).map(|_| Embed { title: "t".into(), description: String::new(), color: None, ..Default::default() }).collect()
    }

    #[test]
    fn chunks_split_at_lines_and_rejoin() {
        let l = long();
        let parts = chunks(&l, 2000);
        assert!(parts.len() > 1 && parts.iter().all(|p| len(p) <= 2000));
        assert_eq!(parts.join("\n"), l);
    }

    #[test]
    fn chunks_5000_chars_with_newlines() {
        let t = vec!["a".repeat(99); 50].join("\n"); // 4999 chars
        let parts = chunks(&t, 2000);
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| len(p) <= 2000));
    }

    #[test]
    fn code_block_is_closed_and_reopened() {
        let parts = chunks(&format!("```\n{}\n```", long()), 2000);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| fences(p) % 2 == 0));
        assert!(parts[1].starts_with("```\n") && parts[0].ends_with("\n```"));
    }

    #[test]
    fn odd_fences_stay_finite_closed_and_within_limits() {
        for t in ["```code".to_string(), format!("```\n{}", long()), format!("{}\n```x", "a".repeat(1995))] {
            let p = chunks(&t, 2000);
            assert!(p.len() < 10 && p.iter().all(|x| len(x) <= 2000 && fences(x) % 2 == 0), "{}", &t[..8]);
        }
    }

    #[test]
    fn no_newline_is_hard_cut() {
        let parts = chunks(&"y".repeat(4500), 2000);
        assert_eq!(parts.iter().map(|p| len(p)).collect::<Vec<_>>(), vec![1992, 1992, 516]);
        assert!(chunks("", 2000).is_empty());
    }

    #[test]
    fn accented_text_never_splits_a_char() {
        let t = "é".repeat(4500);
        let parts = chunks(&t, 2000);
        assert!(parts.len() > 1 && parts.iter().all(|p| len(p) <= 2000 && p.chars().all(|c| c == 'é')));
        assert_eq!(parts.concat(), t);
        assert_eq!(cut(&"é".repeat(300), 256).chars().count(), 256);
    }

    #[test]
    fn embed_splits_30_fields_25_plus_5() {
        let fields: Vec<Field> = (0..30).map(|i| Field { name: format!("f{i}"), value: "v".into(), inline: false }).collect();
        let e = embed("T", COLOR_INFO, None, &fields, None);
        assert_eq!(e.iter().map(|x| x.fields.len()).collect::<Vec<_>>(), vec![25, 5]);
    }

    #[test]
    fn embed_long_field_value_continues_in_extra_fields() {
        let f = [Field { name: "gros".into(), value: "y".repeat(3000), inline: false }];
        let e = embed("T", COLOR_INFO, None, &f, Some("pied"));
        assert!(e[0].fields.len() == 3 && e[0].fields.iter().all(|f| len(&f.value) <= 1024));
        assert_eq!(e[0].fields[1].name, "gros (suite)");
        assert_eq!(e[0].footer.as_deref(), Some("pied"));
    }

    #[test]
    fn embed_9000_char_description_makes_3_embeds() {
        let e = embed("Titre", COLOR_INFO, Some(&"d".repeat(9000)), &[], None);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].title, "Titre");
        assert!(e[1].title.ends_with(" (suite)") && e[2].title.ends_with(" (suite)"));
        assert!(e.iter().all(|x| len(x.description.as_deref().unwrap()) <= 4096));
        assert_eq!(cut(&"t".repeat(300), 256).chars().last(), Some('…'));
    }

    #[test]
    fn twelve_small_embeds_make_two_messages() {
        let m = messages(None, &small(12));
        assert_eq!(m.iter().map(|x| x.embeds.len()).collect::<Vec<_>>(), vec![10, 2]);
        assert!(m.iter().all(|x| x.content.is_none()) && m[0].embeds[0].color == COLOR_INFO);
    }

    #[test]
    fn text_then_embeds_never_share_a_message() {
        let m = messages(Some("texte"), &small(1));
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].content.as_deref(), Some("texte"));
        assert_eq!(m[1].embeds.len(), 1);
    }

    #[test]
    fn embeds_group_by_6000_chars() {
        let big = Embed { title: "t".into(), description: "d".repeat(3500), color: Some(1), ..Default::default() };
        let m = messages(None, &[big.clone(), big.clone(), big]);
        assert_eq!(m.iter().map(|x| x.embeds.len()).collect::<Vec<_>>(), vec![1, 1, 1]);
        assert_eq!(m[0].embeds[0].color, 1);
    }

    #[test]
    fn card_with_30_fields_and_footer() {
        let fields = (0..30).map(|i| bobby_proto::msg::EmbedField { name: format!("f{i}"), value: "v".into(), inline: true }).collect();
        let e = Embed { title: "T".into(), fields, footer: Some("pied".into()), ..Default::default() };
        let m = messages(None, &[e]);
        let embeds: Vec<&EmbedOut> = m.iter().flat_map(|x| x.embeds.iter()).collect();
        assert_eq!(embeds.iter().map(|x| x.fields.len()).collect::<Vec<_>>(), vec![25, 5]);
        assert_eq!(embeds[1].footer.as_deref(), Some("pied"));
        assert!(embeds[0].footer.is_none() && embeds[0].fields[0].inline);
    }

    #[test]
    fn serenity_message_never_pings() {
        let m = messages(Some("@everyone"), &[]);
        let v = serde_json::to_value(to_serenity(&m[0])).unwrap();
        assert_eq!(v["allowed_mentions"]["parse"], serde_json::json!([]));
    }
}
