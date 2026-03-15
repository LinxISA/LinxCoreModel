use anyhow::{Context, Result, bail};
use goblin::elf::{Elf, program_header::PT_INTERP, program_header::PT_LOAD};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentImage {
    pub vaddr: u64,
    pub mem_size: u64,
    pub file_size: u64,
    pub flags: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadedElf {
    pub path: PathBuf,
    pub entry: u64,
    pub little_endian: bool,
    pub bits: u8,
    pub machine: u16,
    pub segments: Vec<SegmentImage>,
}

impl LoadedElf {
    pub fn image_name(&self) -> String {
        self.path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("a.out")
            .to_string()
    }
}

pub fn load_static_elf(path: impl AsRef<Path>) -> Result<LoadedElf> {
    let path = path.as_ref();
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let elf =
        Elf::parse(&bytes).with_context(|| format!("failed to parse ELF {}", path.display()))?;

    if elf.is_64 != true {
        bail!(
            "expected 64-bit ELF, found 32-bit image at {}",
            path.display()
        );
    }
    if !elf.little_endian {
        bail!("expected little-endian ELF at {}", path.display());
    }
    if elf.program_headers.iter().any(|ph| ph.p_type == PT_INTERP) {
        bail!(
            "dynamic interpreter segments are not supported yet; expected static user ELF at {}",
            path.display()
        );
    }

    let mut segments = Vec::new();
    for ph in elf.program_headers.iter().filter(|ph| ph.p_type == PT_LOAD) {
        let start = ph.p_offset as usize;
        let end = start + ph.p_filesz as usize;
        let data = bytes
            .get(start..end)
            .with_context(|| format!("segment outside ELF image for {}", path.display()))?
            .to_vec();
        segments.push(SegmentImage {
            vaddr: ph.p_vaddr,
            mem_size: ph.p_memsz,
            file_size: ph.p_filesz,
            flags: ph.p_flags,
            data,
        });
    }

    if segments.is_empty() {
        bail!("ELF contains no PT_LOAD segments at {}", path.display());
    }

    Ok(LoadedElf {
        path: path.to_path_buf(),
        entry: elf.entry,
        little_endian: elf.little_endian,
        bits: 64,
        machine: elf.header.e_machine,
        segments,
    })
}
