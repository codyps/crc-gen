//! Candidate metadata is independent of simplification ratings. Unknown or
//! tentative HD entries must never pass a quality filter as proven bounds.
use crate::rating::{parse_order, Result};
use crate::CrcSpec;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn polynomial(text: &str) -> Result<u64> {
    let text = text
        .strip_prefix("0x")
        .ok_or("polynomial must use 0x hexadecimal notation")?;
    u64::from_str_radix(text, 16).map_err(|_| "invalid or oversized polynomial".into())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HdBound {
    pub distance: u32,
    /// Decimal string, preserving 64-bit lengths in JSON tools using doubles.
    pub max_data_bits: Option<String>,
    pub status: String,
    pub raw: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub width: u8,
    pub polynomial: String,
    pub koopman: Option<String>,
    pub orientation: String,
    pub hd: Vec<HdBound>,
    pub source: Option<String>,
    pub source_sha256: Option<String>,
    pub raw_row: Option<String>,
}

impl Candidate {
    pub fn spec(&self, bits: u8, order: &str) -> Result<CrcSpec> {
        CrcSpec::new(
            self.width,
            polynomial(&self.polynomial)?,
            bits,
            parse_order(order)?,
        )
        .map_err(|error| error.to_string())
    }
    pub fn validate(&self) -> Result<()> {
        self.spec(8, "msb")?;
        if self.polynomial != format!("{:#x}", polynomial(&self.polynomial)?) {
            return Err("noncanonical polynomial encoding".into());
        }
        let mut distances = BTreeSet::new();
        for bound in &self.hd {
            if bound.distance < 3 || !distances.insert(bound.distance) {
                return Err("invalid or duplicate HD distance".into());
            }
            match bound.status.as_str() {
                "exact" | "lower_bound" => {
                    bound
                        .max_data_bits
                        .as_ref()
                        .ok_or("missing HD bound")?
                        .parse::<u64>()
                        .map_err(|_| "invalid HD length")?;
                }
                "unknown" => {
                    if bound.max_data_bits.is_some() {
                        return Err("unknown HD cannot carry a confirmed bound".into());
                    }
                }
                _ => return Err("unknown HD confidence".into()),
            }
        }
        Ok(())
    }
    pub fn meets(&self, hd: u32, length: u64) -> bool {
        self.hd.iter().any(|bound| {
            bound.distance >= hd
                && matches!(bound.status.as_str(), "exact" | "lower_bound")
                && bound
                    .max_data_bits
                    .as_ref()
                    .and_then(|n| n.parse::<u64>().ok())
                    .is_some_and(|n| n >= length)
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub url: String,
    pub sha256: String,
    pub retrieved_utc: String,
    pub file: String,
    pub license: String,
    pub attribution: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalogue {
    pub version: u32,
    pub sources: Vec<Source>,
    pub candidates: Vec<Candidate>,
}

impl Catalogue {
    pub fn verify_sources(&self, directory: &std::path::Path) -> Result<()> {
        for source in &self.sources {
            let path = std::path::Path::new(&source.file);
            if !path
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                return Err("source snapshot path must be relative to the catalogue".into());
            }
            let bytes = std::fs::read(directory.join(path)).map_err(|e| e.to_string())?;
            if sha256(&bytes) != source.sha256 {
                return Err(format!(
                    "source snapshot checksum mismatch: {}",
                    source.file
                ));
            }
        }
        Ok(())
    }
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let catalog: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        if catalog.version != 1 {
            return Err("unsupported catalogue version".into());
        }
        let mut keys = BTreeSet::new();
        let sources: BTreeMap<_, _> = catalog
            .sources
            .iter()
            .map(|source| (&source.url, &source.sha256))
            .collect();
        for candidate in &catalog.candidates {
            candidate.validate()?;
            if !keys.insert((candidate.width, polynomial(&candidate.polynomial)?)) {
                return Err("duplicate catalogue polynomial".into());
            }
            let source = candidate
                .source
                .as_ref()
                .ok_or("catalogue candidate needs source attribution")?;
            if sources.get(source).copied() != candidate.source_sha256.as_ref() {
                return Err("candidate source checksum is missing or mismatched".into());
            }
        }
        Ok(catalog)
    }
}

pub fn exhaustive(
    width: u8,
    include_even: bool,
    catalog: Option<&Catalogue>,
) -> Result<Vec<Candidate>> {
    if !matches!(width, 8 | 16) {
        return Err("exhaustive search is limited to CRC-8 and CRC-16".into());
    }
    let known: BTreeMap<_, _> = catalog
        .into_iter()
        .flat_map(|c| &c.candidates)
        .filter(|c| c.width == width)
        .map(|c| (c.polynomial.clone(), c.clone()))
        .collect();
    let mut result = Vec::new();
    for value in 0..1u64 << width {
        if !include_even && value & 1 == 0 {
            continue;
        }
        let polynomial = format!("{value:#x}");
        result.push(known.get(&polynomial).cloned().unwrap_or(Candidate {
            width,
            polynomial,
            koopman: None,
            orientation: "enumerated".into(),
            hd: vec![],
            source: None,
            source_sha256: None,
            raw_row: None,
        }));
    }
    Ok(result)
}
