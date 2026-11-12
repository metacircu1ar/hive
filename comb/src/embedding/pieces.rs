//! An input over its embedding model's limit is embedded in overlapping pieces.
//! Each piece repeats the input's title and heading; the chunk's vector is the
//! length-weighted mean of the pieces' vectors. Inputs that fit never reach
//! here, so their vectors are unchanged.
use super::{BATCH_SIZE, Embedder, normalize};
use crate::{Error, Result};

/// Provider-side halvings after any local byte-budget preparation.
const MAX_DEPTH: usize = 3;
/// Bound local preparation and all later leaves, including completed pieces.
const MAX_PIECES: usize = 64;
/// A body piece shorter than this many bytes is not split again.
const MIN_PIECE_BYTES: usize = 64;
/// Bytes a piece of one unbroken run of text shares with its neighbor.
const RUN_OVERLAP_BYTES: usize = 16;

/// A row can be rejected without failing the enclosing transport batch.
#[derive(Debug)]
pub(super) enum Outcome {
    Embedded(Vec<f32>),
    Rejected(Error),
}
impl Outcome {
    pub(super) fn into_result(self) -> Result<Vec<f32>> {
        match self {
            Self::Embedded(vector) => Ok(vector),
            Self::Rejected(error) => Err(error),
        }
    }
}

/// Embed `input` (`title\nheading\nbody`, without the document prefix) in
/// pieces of its body. The outer error fails the batch, as a transport error
/// does; the inner result is the row's: its vector, or the first rejection or
/// `rejected` when the body cannot be split small enough.
pub(super) fn embed(
    embedder: &mut (impl Embedder + ?Sized),
    input: &str,
    rejected: Error,
) -> Result<Outcome> {
    let mut parts = input.splitn(3, '\n');
    let (Some(title), Some(heading), Some(body)) = (parts.next(), parts.next(), parts.next())
    else {
        return Ok(Outcome::Rejected(rejected));
    };
    let header = format!("{}{title}\n{heading}\n", embedder.space().document_prefix);
    let dimensions = embedder.space().dimensions;
    let local = embedder
        .input_byte_limit()
        .filter(|limit| header.len() + body.len() > *limit);
    let mut pending = if let Some(limit) = local {
        let Some(pieces) = byte_pieces(body, header.len(), limit) else {
            return Ok(Outcome::Rejected(rejected));
        };
        pieces
    } else {
        vec![body.to_owned()]
    };
    let mut finished: Vec<(Vec<f32>, usize)> = Vec::new();
    for round in 0..MAX_DEPTH + usize::from(local.is_some()) {
        let pieces = if round == 0 && local.is_some() {
            std::mem::take(&mut pending)
        } else {
            let Some(pieces) = split(&pending) else {
                return Ok(Outcome::Rejected(rejected));
            };
            pieces
        };
        if pieces.len() + finished.len() > MAX_PIECES {
            return Ok(Outcome::Rejected(rejected));
        }
        let results = infer(embedder, &header, &pieces)?;
        pending.clear();
        for (piece, result) in pieces.into_iter().zip(results) {
            match result {
                Ok(vector) => finished.push((normalize(vector, dimensions)?, piece.len())),
                Err(error) if error.is_input_rejection() => pending.push(piece),
                Err(error) => return Ok(Outcome::Rejected(error)),
            }
        }
        if pending.is_empty() {
            return mean(&finished, dimensions).map(Outcome::Embedded);
        }
    }
    Ok(Outcome::Rejected(rejected))
}

fn infer(
    embedder: &mut (impl Embedder + ?Sized),
    header: &str,
    pieces: &[String],
) -> Result<Vec<Result<Vec<f32>>>> {
    let inputs = pieces
        .iter()
        .map(|piece| format!("{header}{piece}"))
        .collect::<Vec<_>>();
    inputs
        .chunks(BATCH_SIZE)
        .map(|batch| {
            let vectors = embedder.embed(batch)?;
            if vectors.len() != batch.len() {
                return Err(Error::embedding_batch_result_count());
            }
            Ok(vectors)
        })
        .collect::<Result<Vec<_>>>()
        .map(|batches| batches.into_iter().flatten().collect())
}

fn split(pending: &[String]) -> Option<Vec<String>> {
    Some(
        pending
            .iter()
            .map(|piece| halves(piece))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .flat_map(|(first, second)| [first, second])
            .collect(),
    )
}

/// Local preparation spends no HTTP calls. Each split strictly shrinks an
/// oversized body and retains the existing minimum-piece floor.
fn byte_pieces(body: &str, header: usize, limit: usize) -> Option<Vec<String>> {
    let available = limit.checked_sub(header)?;
    let mut pieces = vec![body.to_owned()];
    while pieces.iter().any(|piece| piece.len() > available) {
        pieces = pieces
            .into_iter()
            .map(|piece| {
                if piece.len() <= available {
                    Some(vec![piece])
                } else {
                    halves(&piece).map(|(first, second)| vec![first, second])
                }
            })
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        if pieces.len() > MAX_PIECES {
            return None;
        }
    }
    Some(pieces)
}

/// The length-weighted mean of unit vectors, normalized.
fn mean(vectors: &[(Vec<f32>, usize)], dimensions: usize) -> Result<Vec<f32>> {
    let mut sum = vec![0_f64; dimensions];
    for (vector, weight) in vectors {
        let weight = crate::convert::usize_f64(*weight);
        for (total, value) in sum.iter_mut().zip(vector) {
            *total += f64::from(*value) * weight;
        }
    }
    normalize(
        sum.into_iter().map(crate::convert::f64_f32).collect(),
        dimensions,
    )
}

/// Two overlapping halves of `piece`, each shorter than it: by words with a
/// tenth of them shared, or for one long run of text by characters. `None`
/// when the piece is too short to split.
fn halves(piece: &str) -> Option<(String, String)> {
    if piece.len() < 2 * MIN_PIECE_BYTES {
        return None;
    }
    let words: Vec<&str> = piece.split_whitespace().collect();
    if words.len() >= 4 {
        let overlap = (words.len() / 10).max(1);
        let middle = words.len() / 2;
        let end = (middle + overlap.div_ceil(2)).min(words.len() - 1);
        let start = middle.saturating_sub(overlap / 2).max(1);
        return Some((words[..end].join(" "), words[start..].join(" ")));
    }
    let boundary = |at: usize, up: bool| {
        let mut at = at.min(piece.len());
        while !piece.is_char_boundary(at) {
            at = if up { at + 1 } else { at - 1 };
        }
        at
    };
    let middle = piece.len() / 2;
    let end = boundary(middle + RUN_OVERLAP_BYTES / 2, true);
    let start = boundary(middle - RUN_OVERLAP_BYTES / 2, false);
    Some((piece[..end].to_owned(), piece[start..].to_owned()))
}

#[cfg(test)]
mod tests;
