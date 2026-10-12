//! Forme des paquets de mise à jour, lue dans leurs octets (donc la même sur
//! tout système, et essayée partout) : AppImage, .deb, installateur NSIS de
//! Windows. Une signature valide ne suffit pas : écrire un .deb à la place
//! de l'AppImage la détruirait, et lancer autre chose qu'un installateur
//! fermerait ChatonLM pour rien.

use crate::updater::UpdateInstaller;

/// Le paquet a la forme attendue : AppImage de type 2 (ELF 64 bits avec la
/// marque « AI\x02 », suivi de son image squashfs), ou archive .deb (ar dont
/// le premier membre est `debian-binary`). Une signature valide ne suffit
/// pas : écrire un .deb à la place de l'AppImage la détruirait.
/// Installateur NSIS : voir [`is_nsis`].
pub(crate) fn has_format(bytes: &[u8], installer: UpdateInstaller) -> bool {
    match installer {
        UpdateInstaller::AppImage => is_appimage(bytes),
        UpdateInstaller::Deb => {
            bytes.starts_with(b"!<arch>\n") && bytes[8..].starts_with(b"debian-binary")
        }
        UpdateInstaller::Nsis => is_nsis(bytes),
    }
}

fn is_appimage(bytes: &[u8]) -> bool {
    const ELF64_LE: [u8; 6] = [0x7f, b'E', b'L', b'F', 2, 1];
    if !bytes.starts_with(&ELF64_LE) || bytes.get(8..11) != Some(b"AI\x02".as_slice()) {
        return false;
    }
    squashfs_offset(bytes)
        .and_then(|offset| bytes.get(offset..offset.checked_add(4)?))
        .is_some_and(|magic| magic == b"hsqs")
}

/// Fin du programme ELF (runtime de l'AppImage), où commence son image :
/// fin de la table des sections ou de la dernière section, la plus loin des
/// deux (calcul du runtime, `appimage_get_elf_size`).
fn squashfs_offset(bytes: &[u8]) -> Option<usize> {
    let u16_at = |at: usize| Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?));
    let u64_at = |at: usize| {
        let value = u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?);
        usize::try_from(value).ok()
    };
    let table = u64_at(0x28)?;
    let entry = usize::from(u16_at(0x3a)?);
    let count = usize::from(u16_at(0x3c)?);
    let table_end = table.checked_add(entry.checked_mul(count)?)?;
    let last = table.checked_add(entry.checked_mul(count.checked_sub(1)?)?)?;
    let last_end =
        u64_at(last.checked_add(0x18)?)?.checked_add(u64_at(last.checked_add(0x20)?)?)?;
    Some(table_end.max(last_end))
}

/// Marque du premier en-tête d'un installateur NSIS (`firstheader` de
/// `exehead/fileform.h`) : `0xDEADBEEF` puis « NullsoftInst », après quatre
/// octets de drapeaux.
const NSIS_MARK: &[u8; 16] = b"\xEF\xBE\xAD\xDENullsoftInst";

/// Pas des données d'un installateur NSIS : son en-tête suit le programme à
/// un multiple de 512 octets (le lanceur le cherche de la même façon).
const NSIS_STEP: usize = 512;

/// Installateur NSIS (celui de Tauri) : programme Windows (« MZ », puis
/// « PE\0\0 » à l'adresse lue en 0x3c, pour x86 ou x64 : le lanceur de NSIS
/// est un programme 32 bits), suivi de l'en-tête de NSIS à un multiple de
/// 512 octets.
fn is_nsis(bytes: &[u8]) -> bool {
    const I386: u16 = 0x014c;
    const AMD64: u16 = 0x8664;
    if !bytes.starts_with(b"MZ") {
        return false;
    }
    let Some(pe) = bytes
        .get(0x3c..0x40)
        .and_then(|at| at.try_into().ok())
        .map(u32::from_le_bytes)
        .and_then(|at| usize::try_from(at).ok())
    else {
        return false;
    };
    let Some(head) = pe.checked_add(6).and_then(|end| bytes.get(pe..end)) else {
        return false;
    };
    let machine = u16::from_le_bytes([head[4], head[5]]);
    if &head[..4] != b"PE\0\0" || (machine != I386 && machine != AMD64) {
        return false;
    }
    // En-tête de NSIS après le programme : jamais dans le premier bloc.
    (NSIS_STEP..bytes.len())
        .step_by(NSIS_STEP)
        .any(|at| bytes.get(at + 4..at + 4 + NSIS_MARK.len()) == Some(NSIS_MARK.as_slice()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Installateur NSIS d'essai (64 Kio) : en-tête « MZ », « PE\0\0 » pour
    /// x86 à l'octet 128, en-tête de NSIS à l'octet 1024. Mêmes octets que
    /// `testNsis()` de scripts/release-manifest.test.mjs.
    pub fn test_nsis() -> Vec<u8> {
        let mut bytes: Vec<u8> = (0..65_536u32)
            .map(|i| ((i * 31 + 13) % 256) as u8)
            .collect();
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&128u32.to_le_bytes());
        bytes[128..134].copy_from_slice(b"PE\0\0\x4c\x01");
        bytes[1024..1028].fill(0);
        bytes[1028..1044].copy_from_slice(NSIS_MARK);
        bytes
    }

    #[test]
    fn a_nsis_installer_is_recognised_by_its_bytes() {
        let nsis = UpdateInstaller::Nsis;
        assert!(has_format(&test_nsis(), nsis));
        // Lanceur 64 bits : accepté aussi.
        let mut x64 = test_nsis();
        x64[132..134].copy_from_slice(&0x8664u16.to_le_bytes());
        assert!(has_format(&x64, nsis));
        // Sans « MZ », sans « PE », autre processeur (ARM64), sans l'en-tête
        // de NSIS, en-tête ailleurs qu'à un multiple de 512, tronqué.
        for (at, value) in [(0, b'X'), (128, b'X'), (132, 0x64), (1030, 0), (1035, b'X')] {
            let mut other = test_nsis();
            other[at] = value;
            assert!(!has_format(&other, nsis), "octet {at}");
        }
        let mut shifted = test_nsis();
        shifted.copy_within(1024..1044, 1100);
        shifted[1024..1044].fill(0);
        assert!(!has_format(&shifted, nsis));
        assert!(!has_format(&test_nsis()[..1040], nsis));
        assert!(!has_format(&test_nsis()[..0x3e], nsis));
        assert!(!has_format(b"", nsis));
        // Un programme Windows quelconque n'est pas un installateur.
        let mut program = test_nsis();
        program[1024..1044].fill(b'.');
        assert!(!has_format(&program, nsis));
        // Jamais pris pour un paquet Linux, et inversement.
        assert!(!has_format(&test_nsis(), UpdateInstaller::AppImage));
        assert!(!has_format(&test_nsis(), UpdateInstaller::Deb));
        assert!(!has_format(b"!<arch>\ndebian-binary   2.0", nsis));
    }
}
