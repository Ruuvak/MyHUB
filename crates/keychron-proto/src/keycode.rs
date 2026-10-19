//! Keycode decoding for VIA protocol 12 (QMK v12 numbering, public values). Returns a
//! small enum, not UI text; [`std::fmt::Display`] gives QMK-style names such as
//! `MO(3)` or `LCTL(KC_A)`.
//!
//! Ranges [OBS keymap-macros.md section 4, golden keymap]: basic 0x0004..=0x00E7, mod
//! wrapped 0x0100..=0x1FFF, MT 0x2000.., LT 0x4000.., LM 0x5000.., TO 0x5200, MO 0x5220,
//! DF 0x5240, TG 0x5260, OSL 0x5280, OSM 0x52A0, TT 0x52C0, NKRO 0x7013, MACRO 0x7700,
//! RGB 0x7820..=0x7833, custom 0x7E00. Names of basic codes above 0x00A4 follow public
//! QMK; only the ones in the golden keymap are checked against the keyboard.

use std::fmt;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Keycode {
    No,
    Transparent,
    /// A basic HID key, `name` is the QMK name such as `KC_A`.
    Basic {
        code: u8,
        name: &'static str,
    },
    /// Modifier wrapped key. `mods` bits: 0 Ctrl, 1 Shift, 2 Alt, 3 Gui, 4 right side.
    Modded {
        mods: u8,
        key: u8,
    },
    ModTap {
        mods: u8,
        key: u8,
    },
    LayerTap {
        layer: u8,
        key: u8,
    },
    LayerMods {
        layer: u8,
        mods: u8,
    },
    To(u8),
    Momentary(u8),
    DefaultLayer(u8),
    Toggle(u8),
    OneShotLayer(u8),
    OneShotMods(u8),
    TapToggle(u8),
    Macro(u8),
    /// QMK RGB key, `name` such as `RGB_TOG`.
    Rgb {
        name: &'static str,
    },
    NkroToggle,
    /// Keyboard specific key 0x7E00 + index. `name` is known for the K2 HE keys 0..=18.
    Custom {
        index: u8,
        name: Option<&'static str>,
    },
    /// Anything else; show as hex.
    Unknown(u16),
}

const LETTERS: [&str; 26] = [
    "KC_A", "KC_B", "KC_C", "KC_D", "KC_E", "KC_F", "KC_G", "KC_H", "KC_I", "KC_J", "KC_K", "KC_L",
    "KC_M", "KC_N", "KC_O", "KC_P", "KC_Q", "KC_R", "KC_S", "KC_T", "KC_U", "KC_V", "KC_W", "KC_X",
    "KC_Y", "KC_Z",
];
const DIGITS: [&str; 10] = [
    "KC_1", "KC_2", "KC_3", "KC_4", "KC_5", "KC_6", "KC_7", "KC_8", "KC_9", "KC_0",
];
const FKEYS: [&str; 24] = [
    "KC_F1", "KC_F2", "KC_F3", "KC_F4", "KC_F5", "KC_F6", "KC_F7", "KC_F8", "KC_F9", "KC_F10",
    "KC_F11", "KC_F12", "KC_F13", "KC_F14", "KC_F15", "KC_F16", "KC_F17", "KC_F18", "KC_F19",
    "KC_F20", "KC_F21", "KC_F22", "KC_F23", "KC_F24",
];
const KEYPAD: [&str; 10] = [
    "KC_P1", "KC_P2", "KC_P3", "KC_P4", "KC_P5", "KC_P6", "KC_P7", "KC_P8", "KC_P9", "KC_P0",
];
const INTL: [&str; 9] = [
    "KC_INT1", "KC_INT2", "KC_INT3", "KC_INT4", "KC_INT5", "KC_INT6", "KC_INT7", "KC_INT8",
    "KC_INT9",
];
const LANG: [&str; 9] = [
    "KC_LNG1", "KC_LNG2", "KC_LNG3", "KC_LNG4", "KC_LNG5", "KC_LNG6", "KC_LNG7", "KC_LNG8",
    "KC_LNG9",
];
const BUTTONS: [&str; 8] = [
    "KC_BTN1", "KC_BTN2", "KC_BTN3", "KC_BTN4", "KC_BTN5", "KC_BTN6", "KC_BTN7", "KC_BTN8",
];

/// QMK name of a basic HID keycode, `None` for undefined values.
pub fn basic_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0x04..=0x1D => LETTERS[usize::from(code - 0x04)],
        0x1E..=0x27 => DIGITS[usize::from(code - 0x1E)],
        0x28 => "KC_ENT",
        0x29 => "KC_ESC",
        0x2A => "KC_BSPC",
        0x2B => "KC_TAB",
        0x2C => "KC_SPC",
        0x2D => "KC_MINS",
        0x2E => "KC_EQL",
        0x2F => "KC_LBRC",
        0x30 => "KC_RBRC",
        0x31 => "KC_BSLS",
        0x32 => "KC_NUHS",
        0x33 => "KC_SCLN",
        0x34 => "KC_QUOT",
        0x35 => "KC_GRV",
        0x36 => "KC_COMM",
        0x37 => "KC_DOT",
        0x38 => "KC_SLSH",
        0x39 => "KC_CAPS",
        0x3A..=0x45 => FKEYS[usize::from(code - 0x3A)],
        0x46 => "KC_PSCR",
        0x47 => "KC_SCRL",
        0x48 => "KC_PAUS",
        0x49 => "KC_INS",
        0x4A => "KC_HOME",
        0x4B => "KC_PGUP",
        0x4C => "KC_DEL",
        0x4D => "KC_END",
        0x4E => "KC_PGDN",
        0x4F => "KC_RGHT",
        0x50 => "KC_LEFT",
        0x51 => "KC_DOWN",
        0x52 => "KC_UP",
        0x53 => "KC_NUM",
        0x54 => "KC_PSLS",
        0x55 => "KC_PAST",
        0x56 => "KC_PMNS",
        0x57 => "KC_PPLS",
        0x58 => "KC_PENT",
        0x59..=0x62 => KEYPAD[usize::from(code - 0x59)],
        0x63 => "KC_PDOT",
        0x64 => "KC_NUBS",
        0x65 => "KC_APP",
        0x66 => "KC_KB_POWER",
        0x67 => "KC_PEQL",
        0x68..=0x73 => FKEYS[usize::from(code - 0x68) + 12],
        0x74 => "KC_EXEC",
        0x75 => "KC_HELP",
        0x76 => "KC_MENU",
        0x77 => "KC_SLCT",
        0x78 => "KC_STOP",
        0x79 => "KC_AGIN",
        0x7A => "KC_UNDO",
        0x7B => "KC_CUT",
        0x7C => "KC_COPY",
        0x7D => "KC_PSTE",
        0x7E => "KC_FIND",
        0x7F => "KC_KB_MUTE",
        0x80 => "KC_KB_VOLUME_UP",
        0x81 => "KC_KB_VOLUME_DOWN",
        0x82 => "KC_LCAP",
        0x83 => "KC_LNUM",
        0x84 => "KC_LSCR",
        0x85 => "KC_PCMM",
        0x87..=0x8F => INTL[usize::from(code - 0x87)],
        0x90..=0x98 => LANG[usize::from(code - 0x90)],
        0xA5 => "KC_PWR",
        0xA6 => "KC_SLEP",
        0xA7 => "KC_WAKE",
        0xA8 => "KC_MUTE",
        0xA9 => "KC_VOLU",
        0xAA => "KC_VOLD",
        0xAB => "KC_MNXT",
        0xAC => "KC_MPRV",
        0xAD => "KC_MSTP",
        0xAE => "KC_MPLY",
        0xAF => "KC_MSEL",
        0xB0 => "KC_EJCT",
        0xB1 => "KC_MAIL",
        0xB2 => "KC_CALC",
        0xB3 => "KC_MYCM",
        0xB4 => "KC_WSCH",
        0xB5 => "KC_WHOM",
        0xB6 => "KC_WBAK",
        0xB7 => "KC_WFWD",
        0xB8 => "KC_WSTP",
        0xB9 => "KC_WREF",
        0xBA => "KC_WFAV",
        0xBB => "KC_MFFD",
        0xBC => "KC_MRWD",
        0xBD => "KC_BRIU",
        0xBE => "KC_BRID",
        0xBF => "KC_CPNL",
        0xC0 => "KC_ASST",
        0xC1 => "KC_MCTL",
        0xC2 => "KC_LPAD",
        0xCD => "KC_MS_U",
        0xCE => "KC_MS_D",
        0xCF => "KC_MS_L",
        0xD0 => "KC_MS_R",
        0xD1..=0xD8 => BUTTONS[usize::from(code - 0xD1)],
        0xD9 => "KC_WH_U",
        0xDA => "KC_WH_D",
        0xDB => "KC_WH_L",
        0xDC => "KC_WH_R",
        0xDD => "KC_ACL0",
        0xDE => "KC_ACL1",
        0xDF => "KC_ACL2",
        0xE0 => "KC_LCTL",
        0xE1 => "KC_LSFT",
        0xE2 => "KC_LALT",
        0xE3 => "KC_LGUI",
        0xE4 => "KC_RCTL",
        0xE5 => "KC_RSFT",
        0xE6 => "KC_RALT",
        0xE7 => "KC_RGUI",
        _ => return None,
    })
}

const RGB_NAMES: [&str; 20] = [
    "RGB_TOG", "RGB_MOD", "RGB_RMOD", "RGB_HUI", "RGB_HUD", "RGB_SAI", "RGB_SAD", "RGB_VAI",
    "RGB_VAD", "RGB_SPI", "RGB_SPD", "RGB_M_P", "RGB_M_B", "RGB_M_R", "RGB_M_SW", "RGB_M_SN",
    "RGB_M_K", "RGB_M_X", "RGB_M_G", "RGB_M_T",
];

/// K2 HE custom keycode names, index = code - 0x7E00 [OBS definition customKeycodes].
const CUSTOM_NAMES: [&str; 19] = [
    "Left Option",
    "Right Option",
    "Left Cmd",
    "Right Cmd",
    "Mission Control",
    "Launch Pad",
    "Task View",
    "File Explorer",
    "Screenshot",
    "Cortana",
    "Siri",
    "Bluetooth Host 1",
    "Bluetooth Host 2",
    "Bluetooth Host 3",
    "2.4G",
    "Battery Level",
    "Profile 1",
    "Profile 2",
    "Profile 3",
];

impl Keycode {
    pub fn decode(code: u16) -> Keycode {
        let low = (code & 0xFF) as u8;
        let hi = ((code >> 8) & 0x1F) as u8;
        match code {
            0x0000 => Keycode::No,
            0x0001 => Keycode::Transparent,
            0x0004..=0x00FF => match basic_name(low) {
                Some(name) => Keycode::Basic { code: low, name },
                None => Keycode::Unknown(code),
            },
            0x0100..=0x1FFF => Keycode::Modded { mods: hi, key: low },
            0x2000..=0x3FFF => Keycode::ModTap { mods: hi, key: low },
            0x4000..=0x4FFF => Keycode::LayerTap {
                layer: hi & 0x0F,
                key: low,
            },
            0x5000..=0x51FF => Keycode::LayerMods {
                layer: ((code >> 5) & 0x0F) as u8,
                mods: (code & 0x1F) as u8,
            },
            0x5200..=0x52DF => {
                let n = (code & 0x1F) as u8;
                match (code >> 5) & 0x7 {
                    0x0 => Keycode::To(n),
                    0x1 => Keycode::Momentary(n),
                    0x2 => Keycode::DefaultLayer(n),
                    0x3 => Keycode::Toggle(n),
                    0x4 => Keycode::OneShotLayer(n),
                    0x5 => Keycode::OneShotMods(n),
                    _ => Keycode::TapToggle(n),
                }
            }
            0x7013 => Keycode::NkroToggle,
            0x7700..=0x770F => Keycode::Macro((code - 0x7700) as u8),
            0x7820..=0x7833 => Keycode::Rgb {
                name: RGB_NAMES[usize::from(code - 0x7820)],
            },
            0x7E00..=0x7EFF => Keycode::Custom {
                index: low,
                name: CUSTOM_NAMES.get(usize::from(low)).copied(),
            },
            _ => Keycode::Unknown(code),
        }
    }
}

fn mod_names(mods: u8) -> Vec<&'static str> {
    let right = mods & 0x10 != 0;
    let names: [(u8, &str, &str); 4] = [
        (0x01, "LCTL", "RCTL"),
        (0x02, "LSFT", "RSFT"),
        (0x04, "LALT", "RALT"),
        (0x08, "LGUI", "RGUI"),
    ];
    names
        .iter()
        .filter(|(bit, _, _)| mods & bit != 0)
        .map(|&(_, l, r)| if right { r } else { l })
        .collect()
}

fn key_name(key: u8) -> String {
    basic_name(key).map_or_else(|| format!("{key:#04x}"), str::to_owned)
}

impl fmt::Display for Keycode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Keycode::No => f.write_str("KC_NO"),
            Keycode::Transparent => f.write_str("KC_TRNS"),
            Keycode::Basic { name, .. } | Keycode::Rgb { name } => f.write_str(name),
            Keycode::Modded { mods, key } => {
                let names = mod_names(mods);
                for n in &names {
                    write!(f, "{n}(")?;
                }
                f.write_str(&key_name(key))?;
                f.write_str(&")".repeat(names.len()))
            }
            Keycode::ModTap { mods, key } => {
                write!(f, "MT({}, {})", mod_names(mods).join("|"), key_name(key))
            }
            Keycode::LayerTap { layer, key } => write!(f, "LT({layer}, {})", key_name(key)),
            Keycode::LayerMods { layer, mods } => {
                write!(f, "LM({layer}, {})", mod_names(mods).join("|"))
            }
            Keycode::To(n) => write!(f, "TO({n})"),
            Keycode::Momentary(n) => write!(f, "MO({n})"),
            Keycode::DefaultLayer(n) => write!(f, "DF({n})"),
            Keycode::Toggle(n) => write!(f, "TG({n})"),
            Keycode::OneShotLayer(n) => write!(f, "OSL({n})"),
            Keycode::OneShotMods(m) => write!(f, "OSM({})", mod_names(m).join("|")),
            Keycode::TapToggle(n) => write!(f, "TT({n})"),
            Keycode::Macro(n) => write!(f, "MACRO({n})"),
            Keycode::NkroToggle => f.write_str("NK_TOGG"),
            Keycode::Custom {
                name: Some(name), ..
            } => f.write_str(name),
            Keycode::Custom { index, name: None } => write!(f, "CUSTOM({index})"),
            Keycode::Unknown(code) => write!(f, "{code:#06x}"),
        }
    }
}

/// QMK `QK_BOOT`, `QK_REBOOT`, `QK_DEBUG_TOGGLE`, `QK_CLEAR_EEPROM` and the rest of the
/// quantum block live here (public QMK v12 values: 0x7C00 boot, 0x7C01 reboot, 0x7C03
/// clear EEPROM). A keymap write never puts anything from this range on a key.
pub const QUANTUM_BLOCK: std::ops::RangeInclusive<u16> = 0x7C00..=0x7DFF;

/// May `code` be written to a key (VIA 05)? Only what this module can name and that is
/// harmless: KC_NO, KC_TRNS, basic HID keys, `RGB_*` and the K2 HE custom keys. The
/// quantum block (bootloader jump, EEPROM wipe, debug) is refused explicitly, then every
/// other code that is not in the list above. The `Err` text says why.
pub fn check_writable(code: u16) -> Result<(), &'static str> {
    if QUANTUM_BLOCK.contains(&code) {
        return Err(
            "quantum block 0x7C00..=0x7DFF (boot, reboot, clear EEPROM, debug) is never written",
        );
    }
    // Power, sleep and wake keys, and the K2 HE radio keys (BT host 1..3, 2.4G at
    // 0x7E0B..=0x7E0E) could cut the keyboard off or the host to sleep.
    if matches!(code, 0x0066 | 0x00A5..=0x00A7) || (0x7E0B..=0x7E0E).contains(&code) {
        return Err("power, sleep, wake and wireless host keys are never written");
    }
    let known = match Keycode::decode(code) {
        Keycode::No | Keycode::Transparent | Keycode::Basic { .. } | Keycode::Rgb { .. } => true,
        Keycode::Custom { name, .. } => name.is_some(),
        _ => false,
    };
    if known {
        Ok(())
    } else {
        Err("not a basic, KC_NO, KC_TRNS, RGB or Keychron keycode")
    }
}

/// Keycode from a hex or decimal number (`0x004C`) or a QMK name (`KC_DEL`, `RGB_MOD`,
/// `KC_NO`, `KC_TRNS`, a K2 HE custom name). Names are compared case-insensitively. The
/// result is not yet checked with [`check_writable`].
pub fn parse_code(text: &str) -> Option<u16> {
    let t = text.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return u16::from_str_radix(hex, 16).ok();
    }
    if t.bytes().all(|b| b.is_ascii_digit()) && !t.is_empty() {
        return t.parse().ok();
    }
    (0..=u16::MAX)
        .filter(|&c| {
            c == 0x0001
                || c <= 0x00FF
                || (0x7820..=0x7833).contains(&c)
                || (0x7E00..=0x7EFF).contains(&c)
        })
        .find(|&c| {
            let k = Keycode::decode(c);
            !matches!(k, Keycode::Unknown(_)) && k.to_string().eq_ignore_ascii_case(t)
        })
}
