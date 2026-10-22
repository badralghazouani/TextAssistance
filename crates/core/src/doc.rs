//! Splits text into paragraphs and tracks which ones changed between snapshots,
//! so only edited paragraphs are sent for checking.

use crate::{hash_text, utf16};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paragraph {
    /// Stable for the lifetime of the paragraph, even when its position moves.
    pub id: u64,
    pub text: String,
    pub hash: u64,
    /// Bumped on every change, so stale responses can be recognised and dropped.
    pub generation: u32,
}

impl Paragraph {
    /// Blank paragraphs have nothing to check.
    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }
}

#[derive(Debug, Default)]
pub struct Document {
    paragraphs: Vec<Paragraph>,
    next_id: u64,
}

impl Document {
    pub fn new() -> Document {
        Document::default()
    }

    pub fn paragraphs(&self) -> &[Paragraph] {
        &self.paragraphs
    }

    pub fn get(&self, id: u64) -> Option<&Paragraph> {
        self.paragraphs.iter().find(|p| p.id == id)
    }

    /// UTF-16 offset of the paragraph's first character in the whole text.
    pub fn offset_of(&self, id: u64) -> Option<u32> {
        let mut offset = 0;
        for p in &self.paragraphs {
            if p.id == id {
                return Some(offset);
            }
            offset += utf16::len(&p.text) + 1; // +1 for the '\n' separator
        }
        None
    }

    /// Replaces the snapshot with `text` and returns the ids of non-blank
    /// paragraphs that are new or changed, in document order.
    pub fn update(&mut self, text: &str) -> Vec<u64> {
        let new_texts: Vec<&str> = text.split('\n').collect();
        let new_hashes: Vec<u64> = new_texts.iter().map(|t| hash_text(t)).collect();
        let old = std::mem::take(&mut self.paragraphs);

        // Paragraphs equal at the start and the end keep their ids untouched.
        let prefix = old
            .iter()
            .zip(&new_hashes)
            .take_while(|(p, h)| p.hash == **h)
            .count();
        let max_suffix = old.len().min(new_texts.len()) - prefix;
        let suffix = old
            .iter()
            .rev()
            .zip(new_hashes.iter().rev())
            .take(max_suffix)
            .take_while(|(p, h)| p.hash == **h)
            .count();

        let mut old_middle: Vec<Option<Paragraph>> = old[prefix..old.len() - suffix]
            .iter()
            .cloned()
            .map(Some)
            .collect();
        let mut result = Vec::with_capacity(new_texts.len());
        let mut dirty = Vec::new();
        result.extend(old[..prefix].iter().cloned());

        for (i, (&text, &hash)) in new_texts[prefix..new_texts.len() - suffix]
            .iter()
            .zip(&new_hashes[prefix..new_hashes.len() - suffix])
            .enumerate()
        {
            // A paragraph that only moved keeps its id and is not re-checked.
            let moved = old_middle
                .iter_mut()
                .find(|slot| slot.as_ref().is_some_and(|p| p.hash == hash))
                .and_then(Option::take);
            let paragraph = if let Some(p) = moved {
                p
            } else if let Some(edited) = old_middle.get_mut(i).and_then(Option::take) {
                Paragraph {
                    id: edited.id,
                    text: text.to_owned(),
                    hash,
                    generation: edited.generation + 1,
                }
            } else {
                self.next_id += 1;
                Paragraph {
                    id: self.next_id,
                    text: text.to_owned(),
                    hash,
                    generation: 0,
                }
            };
            let changed = !old.iter().any(|p| p.id == paragraph.id && p.hash == hash);
            if changed && !paragraph.is_blank() {
                dirty.push(paragraph.id);
            }
            result.push(paragraph);
        }

        result.extend(old[old.len() - suffix..].iter().cloned());
        self.paragraphs = result;
        dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(doc: &Document) -> Vec<&str> {
        doc.paragraphs().iter().map(|p| p.text.as_str()).collect()
    }

    #[test]
    fn first_update_marks_every_non_blank_paragraph_dirty() {
        let mut doc = Document::new();
        let dirty = doc.update("One.\n\nTwo.");
        assert_eq!(texts(&doc), ["One.", "", "Two."]);
        assert_eq!(dirty.len(), 2);
    }

    #[test]
    fn unchanged_text_is_not_dirty() {
        let mut doc = Document::new();
        doc.update("One.\nTwo.");
        assert!(doc.update("One.\nTwo.").is_empty());
    }

    #[test]
    fn editing_one_paragraph_keeps_its_id_and_bumps_generation() {
        let mut doc = Document::new();
        doc.update("One.\nTwo.\nThree.");
        let id = doc.paragraphs()[1].id;
        let dirty = doc.update("One.\nTwo!\nThree.");
        assert_eq!(dirty, [id]);
        assert_eq!(doc.paragraphs()[1].id, id);
        assert_eq!(doc.paragraphs()[1].generation, 1);
    }

    #[test]
    fn inserting_a_paragraph_only_dirties_the_new_one() {
        let mut doc = Document::new();
        doc.update("One.\nThree.");
        let ids: Vec<u64> = doc.paragraphs().iter().map(|p| p.id).collect();
        let dirty = doc.update("One.\nTwo.\nThree.");
        assert_eq!(dirty.len(), 1);
        assert_eq!(doc.get(dirty[0]).unwrap().text, "Two.");
        assert_eq!(doc.paragraphs()[0].id, ids[0]);
        assert_eq!(doc.paragraphs()[2].id, ids[1]);
    }

    #[test]
    fn swapped_paragraphs_are_not_rechecked() {
        let mut doc = Document::new();
        doc.update("A.\nB.\nC.\nD.");
        assert!(doc.update("A.\nC.\nB.\nD.").is_empty());
    }

    #[test]
    fn offsets_count_utf16_and_newlines() {
        let mut doc = Document::new();
        doc.update("é😀\nnext");
        let second = doc.paragraphs()[1].id;
        assert_eq!(doc.offset_of(second), Some(4));
    }
}
