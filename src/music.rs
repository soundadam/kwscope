//! 十二平均律：MIDI 编号 ↔ 频率，音名，音阶。

pub const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// A4 = MIDI 69。
pub fn midi_to_freq(midi: f32, a4: f32) -> f32 {
    a4 * ((midi - 69.0) / 12.0).exp2()
}

pub fn freq_to_midi(f: f32, a4: f32) -> f32 {
    69.0 + 12.0 * (f / a4).log2()
}

/// 最近的半音及偏差（音分，正为偏高）。
pub fn nearest_note(f: f32, a4: f32) -> (i32, f32) {
    let m = freq_to_midi(f, a4);
    let n = m.round();
    (n as i32, (m - n) * 100.0)
}

/// 科学音高记号，中央 C = C4。
pub fn note_name(midi: i32) -> String {
    format!(
        "{}{}",
        NOTE_NAMES[midi.rem_euclid(12) as usize],
        midi.div_euclid(12) - 1
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scale {
    Chromatic,
    Major,
    NaturalMinor,
    HarmonicMinor,
    MelodicMinor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    MajorPentatonic,
    MinorPentatonic,
    Blues,
    WholeTone,
}

impl Scale {
    pub const ALL: [Self; 13] = [
        Self::Chromatic,
        Self::Major,
        Self::NaturalMinor,
        Self::HarmonicMinor,
        Self::MelodicMinor,
        Self::Dorian,
        Self::Phrygian,
        Self::Lydian,
        Self::Mixolydian,
        Self::MajorPentatonic,
        Self::MinorPentatonic,
        Self::Blues,
        Self::WholeTone,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Chromatic => "半音阶",
            Self::Major => "大调",
            Self::NaturalMinor => "自然小调",
            Self::HarmonicMinor => "和声小调",
            Self::MelodicMinor => "旋律小调",
            Self::Dorian => "多利亚",
            Self::Phrygian => "弗里几亚",
            Self::Lydian => "利底亚",
            Self::Mixolydian => "混合利底亚",
            Self::MajorPentatonic => "大调五声",
            Self::MinorPentatonic => "小调五声",
            Self::Blues => "布鲁斯",
            Self::WholeTone => "全音阶",
        }
    }

    /// 相对主音的半音数，升序，以 0 开头。
    pub fn intervals(self) -> &'static [u8] {
        match self {
            Self::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            Self::Major => &[0, 2, 4, 5, 7, 9, 11],
            Self::NaturalMinor => &[0, 2, 3, 5, 7, 8, 10],
            Self::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Self::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            Self::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Self::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Self::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Self::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Self::MajorPentatonic => &[0, 2, 4, 7, 9],
            Self::MinorPentatonic => &[0, 3, 5, 7, 10],
            Self::Blues => &[0, 3, 5, 6, 7, 10],
            Self::WholeTone => &[0, 2, 4, 6, 8, 10],
        }
    }
}

pub struct ScaleLine {
    pub midi: i32,
    pub freq: f32,
    pub is_tonic: bool,
}

/// `[f_min, f_max]` 内属于该音阶的所有音，按频率升序。`tonic` 是音级 0..12（0 = C）。
pub fn scale_lines(scale: Scale, tonic: u8, a4: f32, f_min: f32, f_max: f32) -> Vec<ScaleLine> {
    let lo = freq_to_midi(f_min, a4).ceil() as i32;
    let hi = freq_to_midi(f_max, a4).floor() as i32;
    (lo..=hi)
        .filter_map(|midi| {
            let rel = (midi - tonic as i32).rem_euclid(12) as u8;
            scale.intervals().contains(&rel).then(|| ScaleLine {
                midi,
                freq: midi_to_freq(midi as f32, a4),
                is_tonic: rel == 0,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn freq_midi_roundtrip() {
        for m in 0..128 {
            let back = freq_to_midi(midi_to_freq(m as f32, 440.0), 440.0);
            assert!((back - m as f32).abs() < 1e-3);
        }
    }

    #[test]
    fn reference_pitch_is_a4_for_any_tuning() {
        for a4 in [415.0, 432.0, 440.0, 442.0] {
            let (m, cents) = nearest_note(a4, a4);
            assert_eq!(note_name(m), "A4");
            assert!(cents.abs() < 1e-3);
        }
    }

    #[test]
    fn cents_are_signed_deviation() {
        let (m, c) = nearest_note(440.0 * (10.0f32 / 1200.0).exp2(), 440.0);
        assert_eq!(m, 69);
        assert!((c - 10.0).abs() < 0.01);
        let (m, c) = nearest_note(440.0 * (-30.0f32 / 1200.0).exp2(), 440.0);
        assert_eq!(m, 69);
        assert!((c + 30.0).abs() < 0.01);
    }

    #[test]
    fn note_names_cross_octave_at_c() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(59), "B3");
        assert_eq!(note_name(0), "C-1");
    }

    #[test]
    fn lines_are_in_range_ascending_and_include_tonic() {
        for scale in Scale::ALL {
            for tonic in 0..12 {
                let lines = scale_lines(scale, tonic, 440.0, 55.0, 4_000.0);
                assert!(lines.windows(2).all(|w| w[0].freq < w[1].freq));
                assert!(lines.iter().all(|l| (55.0..=4_000.0).contains(&l.freq)));
                assert!(lines
                    .iter()
                    .any(|l| l.is_tonic && l.midi.rem_euclid(12) == tonic as i32));
            }
        }
    }

    #[test]
    fn changing_tonic_transposes_pitch_classes() {
        let classes = |scale, tonic| -> BTreeSet<i32> {
            scale_lines(scale, tonic, 440.0, 20.0, 20_000.0)
                .iter()
                .map(|l| l.midi.rem_euclid(12))
                .collect()
        };
        for scale in Scale::ALL {
            let c = classes(scale, 0);
            assert_eq!(c.len(), scale.intervals().len());
            let shifted: BTreeSet<i32> = c.iter().map(|pc| (pc + 7) % 12).collect();
            assert_eq!(classes(scale, 7), shifted);
        }
    }
}
