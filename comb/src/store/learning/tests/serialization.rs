//! Exact pre-borrow evidence representation, including absent fields and escaped text.
use super::*;

// Frozen S8 field types and order: this owns all values as the prior writer did.
#[derive(Serialize)]
struct OwnedEvidence {
    recopy: bool,
    original_markdown: Option<String>,
    compiled: Option<OwnedCompiled>,
    entries: Vec<Entry>,
}
#[derive(Serialize)]
struct OwnedCompiled {
    title: String,
    kind: String,
    tags: Vec<String>,
    aliases: Vec<String>,
    summary: String,
}

#[test]
fn borrowed_evidence_keeps_owned_serialization_bytes() -> Result<()> {
    let (_directory, comb, note) = setup();
    let source = comb.get(&note.parse()?)?;
    for first in [false, true] {
        for changed in [false, true] {
            for recopy in [false, true] {
                let page = &source.page;
                let old = OwnedEvidence {
                    recopy,
                    original_markdown: first.then(|| source.markdown.clone()),
                    compiled: (!first && changed).then(|| OwnedCompiled {
                        title: page.metadata.authored.title.clone(),
                        kind: page.metadata.authored.kind.clone(),
                        tags: page.metadata.authored.tags.clone(),
                        aliases: page.metadata.authored.aliases.clone(),
                        summary: page.compiled.clone(),
                    }),
                    entries: if first {
                        Vec::new()
                    } else {
                        page.timeline.clone()
                    },
                };
                let borrowed = CopiedEvidence {
                    recopy,
                    original_markdown: first.then_some(Cow::Borrowed(source.markdown.as_str())),
                    compiled: (!first && changed).then(|| Compiled::from(page)),
                    entries: Cow::Borrowed(if first { &[] } else { &page.timeline }),
                };
                let bytes = serde_json::to_vec(&borrowed)?;
                assert_eq!(bytes, serde_json::to_vec(&old)?);
                let decoded: CopiedEvidence<'static> = serde_json::from_slice(&bytes)?;
                assert_eq!(bytes, serde_json::to_vec(&decoded)?);
            }
        }
    }
    Ok(())
}
