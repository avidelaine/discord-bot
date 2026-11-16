//! Discord formatting the brain still does (port of `bot/format.js`): colors and monospace tables.
//! Splitting to Discord's limits (`chunks`, `embed`, `messages`) is the Discord client's job now: the brain sends
//! one logical card (`Embed` with fields and footer) and the client splits it exactly like format.js.
use bobby_proto::msg::{Embed, EmbedField};

pub const INFO: u32 = 0x5865f2;
pub const TODO: u32 = 0x57f287;
pub const RIGHTS: u32 = 0xfee75c;
pub const NICKNAMES: u32 = 0xeb459e;
pub const CLAUDE: u32 = 0xd97757;

/// `embed({ title, color, description, fields, footer })` before splitting. Fields: (name, value, inline).
pub fn card(title: &str, color: u32, description: Option<&str>, fields: &[(&str, &str, bool)], footer: Option<&str>) -> Embed {
    Embed {
        title: title.into(),
        description: description.unwrap_or("").into(),
        color: Some(color),
        fields: fields.iter().map(|(n, v, i)| EmbedField { name: n.to_string(), value: if v.is_empty() { "-".into() } else { v.to_string() }, inline: *i }).collect(),
        footer: footer.map(Into::into),
    }
}

/// Monospace table with aligned columns, in a code block. Widths count chars (JS: UTF-16 units; same without emoji).
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let all: Vec<Vec<String>> = std::iter::once(headers.iter().map(|h| h.to_string()).collect()).chain(rows.iter().cloned()).collect();
    let w: Vec<usize> = (0..headers.len()).map(|i| all.iter().map(|r| r.get(i).map_or(0, |c| c.chars().count())).max().unwrap_or(0)).collect();
    let fmt = |r: &Vec<String>| {
        r.iter().enumerate().map(|(i, c)| format!("{c}{}", " ".repeat(w[i].saturating_sub(c.chars().count())))).collect::<Vec<_>>().join("  ").trim_end().to_string()
    };
    let mut out = vec!["```".to_string(), fmt(&all[0]), w.iter().map(|n| "-".repeat(*n)).collect::<Vec<_>>().join("  ")];
    out.extend(all[1..].iter().map(fmt));
    out.push("```".into());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_table() {
        let t = table(&["Personne", "Surnoms"], &[vec!["Ana".into(), "la Reine".into()], vec!["Zoé".into(), "le Grec, Zo".into()]]);
        assert_eq!(t, "```\nPersonne  Surnoms\n--------  -----------\nAna       la Reine\nZoé       le Grec, Zo\n```");
    }
}
