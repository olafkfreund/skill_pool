use anyhow::Result;
use unicode_segmentation::UnicodeSegmentation;

use crate::client::{Client, Skill};
use crate::config::Config;

const DESC_MAX_GRAPHEMES: usize = 60;
const TAGS_MAX_GRAPHEMES: usize = 32;

pub async fn run(
    cfg: &Config,
    query: Option<&str>,
    tags: &[String],
    limit: Option<u32>,
    json: bool,
    semantic: Option<&str>,
    min_similarity: Option<f32>,
) -> Result<()> {
    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;
    let skills = client
        .list_skills(query, tags, limit, semantic, min_similarity)
        .await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&skills)?);
        return Ok(());
    }

    if skills.is_empty() {
        println!("no skills found");
        return Ok(());
    }

    render_table(&skills, semantic.is_some());
    Ok(())
}

fn render_table(skills: &[Skill], show_similarity: bool) {
    let slug_w = skills
        .iter()
        .map(|s| s.slug.len())
        .max()
        .unwrap_or(4)
        .clamp(4, 40);
    let ver_w = skills
        .iter()
        .map(|s| s.version.len())
        .max()
        .unwrap_or(7)
        .max(7);

    let header = if show_similarity {
        format!(
            "{:<6}  {:<slug_w$}  {:<ver_w$}  {:<DESC_MAX_GRAPHEMES$}  TAGS",
            "MATCH", "SLUG", "VERSION", "DESCRIPTION",
        )
    } else {
        format!(
            "{:<slug_w$}  {:<ver_w$}  {:<DESC_MAX_GRAPHEMES$}  TAGS",
            "SLUG", "VERSION", "DESCRIPTION",
        )
    };
    println!("{header}");
    println!("{}", "-".repeat(header.len().min(160)));

    for s in skills {
        let desc = truncate_to(&s.description, DESC_MAX_GRAPHEMES);
        let tags = truncate_to(&s.tags.join(", "), TAGS_MAX_GRAPHEMES);
        if show_similarity {
            let sim = s
                .similarity
                .map(|v| format!("{:>4.0}%", (v * 100.0).clamp(0.0, 100.0)))
                .unwrap_or_else(|| "    —".into());
            println!(
                "{:<6}  {:<slug_w$}  {:<ver_w$}  {:<DESC_MAX_GRAPHEMES$}  {}",
                sim, s.slug, s.version, desc, tags,
            );
        } else {
            println!(
                "{:<slug_w$}  {:<ver_w$}  {:<DESC_MAX_GRAPHEMES$}  {}",
                s.slug, s.version, desc, tags,
            );
        }
    }
}

/// Truncate to N graphemes, append `…` if anything was dropped.
fn truncate_to(s: &str, max: usize) -> String {
    let graphemes: Vec<&str> = s.graphemes(true).collect();
    if graphemes.len() <= max {
        return s.to_string();
    }
    let mut out: String = graphemes
        .iter()
        .take(max.saturating_sub(1))
        .copied()
        .collect();
    out.push('\u{2026}'); // …
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_long_descriptions() {
        let s = "a".repeat(100);
        let t = truncate_to(&s, 20);
        assert_eq!(t.graphemes(true).count(), 20);
        assert!(t.ends_with('\u{2026}'));
    }

    #[test]
    fn keeps_short_strings_untouched() {
        assert_eq!(truncate_to("short", 60), "short");
    }
}
