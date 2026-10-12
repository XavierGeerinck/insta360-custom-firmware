//! Just enough ELF to stop `waved-pack` from shipping the wrong binary: it
//! must be 64-bit little-endian AArch64 (the Wave), and stripped (`/data`
//! has no room for symbols).

const EM_AARCH64: u16 = 183;
const SHT_SYMTAB: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElfInfo {
    pub aarch64: bool,
    /// A symbol table or debug sections are present.
    pub unstripped: bool,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// `None` when `bytes` is not a 64-bit little-endian ELF file.
pub fn inspect(bytes: &[u8]) -> Option<ElfInfo> {
    if bytes.get(..6)? != b"\x7fELF\x02\x01" {
        return None;
    }
    let machine = u16_at(bytes, 0x12)?;
    let shoff = usize::try_from(u64_at(bytes, 0x28)?).ok()?;
    let shentsize = usize::from(u16_at(bytes, 0x3a)?);
    let shnum = usize::from(u16_at(bytes, 0x3c)?);
    let shstrndx = usize::from(u16_at(bytes, 0x3e)?);
    // The section table sits at the end of a linked file: a cut file loses it.
    if shoff.checked_add(shnum.checked_mul(shentsize)?)? > bytes.len() {
        return None;
    }
    let section = |i: usize| -> Option<(u32, u32, usize, usize)> {
        let at = shoff.checked_add(i.checked_mul(shentsize)?)?;
        Some((
            u32_at(bytes, at)?,
            u32_at(bytes, at + 4)?,
            usize::try_from(u64_at(bytes, at + 0x18)?).ok()?,
            usize::try_from(u64_at(bytes, at + 0x20)?).ok()?,
        ))
    };
    let mut unstripped = false;
    if shnum > 0 && shentsize >= 0x28 {
        let (_, _, names_at, names_len) = section(shstrndx)?;
        let names = bytes.get(names_at..names_at.checked_add(names_len)?)?;
        for i in 0..shnum {
            let (name, kind, _, _) = section(i)?;
            let name = names.get(name as usize..).unwrap_or(&[]);
            let name = &name[..name.iter().position(|b| *b == 0).unwrap_or(name.len())];
            if kind == SHT_SYMTAB || name == b".symtab" || name.starts_with(b".debug") {
                unstripped = true;
            }
        }
    }
    Some(ElfInfo {
        aarch64: machine == EM_AARCH64,
        unstripped,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal ELF64 LE file with the named sections (and `.shstrtab`).
    pub(crate) fn elf(machine: u16, sections: &[(&str, u32)], tail: &[u8]) -> Vec<u8> {
        let mut names = vec![0u8];
        let mut headers = vec![[0u8; 0x40]]; // the null section
        let mut entries: Vec<(u32, u32)> = Vec::new();
        for (name, kind) in sections.iter().chain([(".shstrtab", 3)].iter()) {
            entries.push((names.len() as u32, *kind));
            names.extend_from_slice(name.as_bytes());
            names.push(0);
        }
        let names_at = 0x40 + tail.len();
        let shoff = names_at + names.len();
        for (name, kind) in &entries {
            let mut header = [0u8; 0x40];
            header[0..4].copy_from_slice(&name.to_le_bytes());
            header[4..8].copy_from_slice(&kind.to_le_bytes());
            header[0x18..0x20].copy_from_slice(&(names_at as u64).to_le_bytes());
            header[0x20..0x28].copy_from_slice(&(names.len() as u64).to_le_bytes());
            headers.push(header);
        }
        let mut out = vec![0u8; 0x40];
        out[..6].copy_from_slice(b"\x7fELF\x02\x01");
        out[0x12..0x14].copy_from_slice(&machine.to_le_bytes());
        out[0x28..0x30].copy_from_slice(&(shoff as u64).to_le_bytes());
        out[0x3a..0x3c].copy_from_slice(&0x40u16.to_le_bytes());
        out[0x3c..0x3e].copy_from_slice(&(headers.len() as u16).to_le_bytes());
        out[0x3e..0x40].copy_from_slice(&((headers.len() - 1) as u16).to_le_bytes());
        out.extend_from_slice(tail);
        out.extend_from_slice(&names);
        for header in headers {
            out.extend_from_slice(&header);
        }
        out
    }

    #[test]
    fn a_stripped_aarch64_binary_is_recognised() {
        let info = inspect(&elf(EM_AARCH64, &[(".text", 1), (".rodata", 1)], b"")).unwrap();
        assert_eq!(
            info,
            ElfInfo {
                aarch64: true,
                unstripped: false
            }
        );
    }

    #[test]
    fn symbols_debug_info_and_other_machines_are_noticed() {
        assert!(
            inspect(&elf(EM_AARCH64, &[(".symtab", 2)], b""))
                .unwrap()
                .unstripped
        );
        assert!(
            inspect(&elf(EM_AARCH64, &[(".debug_info", 1)], b""))
                .unwrap()
                .unstripped
        );
        assert!(!inspect(&elf(62, &[], b"")).unwrap().aarch64, "x86-64");
        assert_eq!(inspect(b"\xcf\xfa\xed\xfe macho"), None);
        assert_eq!(inspect(b"\x7fELF\x01\x01 32-bit"), None);
        let mut cut = elf(EM_AARCH64, &[(".text", 1)], b"");
        cut.truncate(cut.len() - 10);
        assert_eq!(inspect(&cut), None);
    }
}
