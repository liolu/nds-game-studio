//! Musiques et bruitages `SSEQ` / `SSAR` du SDK Nitro, joués avec leurs instruments `SBNK` et leurs sons `SWAR` : un petit
//! synthétiseur qui suit les commandes des pistes comme la DS (mise à jour toutes les 5,2 ms, 240 unités de tempo par
//! pas). Pas de glissement entre les notes (portamento) ni de modulation du volume : c'est un aperçu.

use std::collections::HashMap;
use std::rc::Rc;

use crate::errors::Result;
use crate::sadl::Sound;
use crate::sdat::{swar_wave, Wave};

const OUTPUT_RATE: u32 = 32000;
/// 64 x 2728 cycles de l'horloge de la DS.
const UPDATE_SECONDS: f64 = 64.0 * 2728.0 / 33_513_982.0;
const MAX_SECONDS: f64 = 240.0;
/// Après la fin des pistes, le temps laissé aux notes pour s'éteindre.
const MAX_TAIL_SECONDS: f64 = 6.0;
const MAX_VOICES: usize = 16;
const MAX_TRACKS: usize = 16;
const MAX_COMMANDS_PER_TICK: usize = 4096;
const SILENCE: i32 = -723;
const AMPL_SHIFT: i32 = 7;
const AMPL_MIN: i32 = SILENCE << AMPL_SHIFT;
const TEMPO_UNITS: i32 = 240;
const MIX_LEVEL: f32 = 0.6;
const PSG_BASE_HZ: f64 = 440.0;
const PSG_STEPS: f64 = 8.0;
/// Un bruit avance bien plus vite qu'une note carrée.
const NOISE_SPEED: f64 = 8.0;
const LFO_SCALE: f64 = 127.0 / 256.0 / 64.0;
const LFO_STEP: f64 = 1.0 / 2048.0;

const PCM: u8 = 1;
const PSG: u8 = 2;
const NOISE: u8 = 3;
const DRUMS: u8 = 16;
const SPLIT: u8 = 17;
const BANK_COUNT_AT: usize = 0x38;
const BANK_RECORDS_AT: usize = 0x3C;
const SPLIT_KEYS: usize = 8;
const REGION_RECORD: usize = 12;

#[derive(Clone, Copy)]
pub struct Region {
    kind: u8,
    wave: u16,
    archive: u16,
    key: u8,
    attack: u8,
    decay: u8,
    sustain: u8,
    release: u8,
    pan: u8,
}

pub enum Instrument {
    Empty,
    Single(Region),
    Drums { low: u8, regions: Vec<Region> },
    Split { keys: [u8; SPLIT_KEYS], regions: Vec<Region> },
}

impl Instrument {
    /// Une note jouable, pour écouter l'instrument seul.
    fn sample_key(&self) -> u8 {
        match self {
            Instrument::Drums { low, .. } => *low,
            _ => 60,
        }
    }

    fn region(&self, key: u8) -> Option<Region> {
        match self {
            Instrument::Empty => None,
            Instrument::Single(region) => Some(*region),
            Instrument::Drums { low, regions } => regions.get(key.checked_sub(*low)? as usize).copied(),
            Instrument::Split { keys, regions } => regions.get(keys.iter().position(|top| *top != 0 && key <= *top)?).copied(),
        }
    }

    pub fn describe(&self) -> String {
        let one = |r: &Region| match r.kind {
            PCM => format!("son {} de l'archive {}", r.wave, r.archive),
            PSG => format!("onde carree {}", r.wave),
            NOISE => "bruit".to_string(),
            other => format!("type {other}"),
        };
        match self {
            Instrument::Empty => "-".to_string(),
            Instrument::Single(region) => one(region),
            Instrument::Drums { regions, .. } => format!("batterie, {} sons", regions.len()),
            Instrument::Split { regions, .. } => format!("{} zones", regions.len()),
        }
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

fn region_at(file: &[u8], kind: u8, at: usize) -> Option<Region> {
    let v = file.get(at + 4..at + 10)?;
    Some(Region { kind, wave: u16_at(file, at)?, archive: u16_at(file, at + 2)?, key: v[0], attack: v[1], decay: v[2], sustain: v[3], release: v[4], pan: v[5] })
}

/// Une suite de zones : type (u16), puis la zone.
fn regions_at(file: &[u8], at: usize, count: usize) -> Vec<Region> {
    (0..count).filter_map(|i| region_at(file, *file.get(at + i * REGION_RECORD)?, at + i * REGION_RECORD + 2)).collect()
}

/// Les instruments d'une banque `SBNK`.
pub fn read_bank(file: &[u8]) -> Result<Vec<Instrument>> {
    if file.get(0..4) != Some(b"SBNK") {
        bail!("pas une banque SBNK");
    }
    let Some(count) = u32_at(file, BANK_COUNT_AT) else { bail!("SBNK trop court") };
    let mut instruments = Vec::new();
    for i in 0..count.min(65536) {
        let record = BANK_RECORDS_AT + i * 4;
        let (Some(&kind), Some(at)) = (file.get(record), u16_at(file, record + 1)) else { break };
        let at = at as usize;
        let instrument = match kind {
            PCM | PSG | NOISE => region_at(file, kind, at).map(Instrument::Single),
            DRUMS => file.get(at..at + 2).map(|range| Instrument::Drums { low: range[0], regions: regions_at(file, at + 2, (range[1] as usize + 1).saturating_sub(range[0] as usize)) }),
            SPLIT => file.get(at..at + SPLIT_KEYS).map(|top| {
                let mut keys = [0; SPLIT_KEYS];
                keys.copy_from_slice(top);
                Instrument::Split { keys, regions: regions_at(file, at + SPLIT_KEYS, keys.iter().filter(|k| **k != 0).count()) }
            }),
            _ => None,
        };
        instruments.push(instrument.unwrap_or(Instrument::Empty));
    }
    Ok(instruments)
}

/// Où commencent les commandes d'un `SSEQ`.
pub fn sequence_data(file: &[u8]) -> Result<&[u8]> {
    if file.get(0..4) != Some(b"SSEQ") {
        bail!("pas une sequence SSEQ");
    }
    match u32_at(file, 0x18).and_then(|at| file.get(at..)) {
        Some(data) => Ok(data),
        None => bail!("SSEQ coupe"),
    }
}

pub struct ArchiveItem {
    pub start: usize,
    pub bank: u16,
    pub volume: u8,
}

/// Les commandes communes et les bruitages d'un `SSAR` (position, banque, volume) ; ceux sans position sont vides.
pub fn read_archive(file: &[u8]) -> Result<(&[u8], Vec<ArchiveItem>)> {
    const RECORD: usize = 12;
    if file.get(0..4) != Some(b"SSAR") {
        bail!("pas une archive SSAR");
    }
    let (Some(data), Some(count)) = (u32_at(file, 0x18), u32_at(file, 0x1C)) else { bail!("SSAR trop court") };
    let Some(commands) = file.get(data..) else { bail!("SSAR coupe") };
    let mut items = Vec::new();
    for i in 0..count.min(65536) {
        let at = 0x20 + i * RECORD;
        let (Some(start), Some(bank), Some(&volume)) = (u32_at(file, at), u16_at(file, at + 4), file.get(at + 6)) else { break };
        if start != 0xFFFF_FFFF {
            items.push(ArchiveItem { start, bank, volume });
        }
    }
    Ok((commands, items))
}

/// Volume (0-127) en dixièmes de décibel.
fn volume_db(value: u8) -> i32 {
    if value == 0 {
        return SILENCE;
    }
    ((40.0 * (value.min(127) as f64 / 127.0).log10() * 10.0) as i32).max(SILENCE)
}

fn attack_level(attack: u8) -> i32 {
    const FAST: [i32; 19] = [0x00, 0x01, 0x05, 0x0E, 0x1A, 0x26, 0x33, 0x3F, 0x49, 0x54, 0x5C, 0x64, 0x6D, 0x74, 0x7B, 0x7F, 0x84, 0x89, 0x8F];
    let attack = attack.min(127);
    if attack >= 0x6D { FAST[(0x7F - attack) as usize] } else { 0xFF - attack as i32 }
}

fn fall_rate(fall: u8) -> i32 {
    match fall.min(127) {
        0x7F => 0xFFFF,
        0x7E => 0x3C00,
        f if f < 0x32 => f as i32 * 2 + 1,
        f => 0x1E00 / (0x7E - f as i32),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Attack,
    Decay,
    Sustain,
    Release,
}

enum Source {
    Wave { wave: Rc<Wave>, position: f64 },
    Square { duty: u16, phase: f64 },
    Noise { state: u16, phase: f64, value: f32 },
}

struct Voice {
    source: Source,
    track: usize,
    key: u8,
    base_key: u8,
    velocity_db: i32,
    region_pan: i32,
    stage: Stage,
    ampl: i32,
    attack: i32,
    decay: i32,
    sustain: i32,
    release: i32,
    /// Pas de séquence restants ; négatif : la note ne s'arrête pas d'elle-même.
    remaining: i64,
    lfo_phase: f64,
    lfo_wait: u16,
    tied: bool,
}

impl Voice {
    fn keeps_playing_forever(&self) -> bool {
        match &self.source {
            Source::Wave { wave, .. } => wave.loop_start.is_some(),
            _ => true,
        }
    }

    fn next_sample(&mut self, step: f64) -> Option<f32> {
        match &mut self.source {
            Source::Wave { wave, position } => {
                let (i, fraction) = (*position as usize, position.fract() as f32);
                let at = |n: usize| wave.samples.get(n).copied();
                let first = at(i)?;
                let next = at(i + 1).or(wave.loop_start.and_then(at)).unwrap_or(first);
                *position += step * wave.rate as f64 / OUTPUT_RATE as f64;
                if *position >= wave.samples.len() as f64 {
                    let start = wave.loop_start?;
                    *position = start as f64 + (*position - wave.samples.len() as f64);
                    if start >= wave.samples.len() {
                        return None;
                    }
                }
                Some((first as f32 + (next as f32 - first as f32) * fraction) / 32768.0)
            }
            Source::Square { duty, phase } => {
                let high = (*phase * PSG_STEPS) as u16 % PSG_STEPS as u16 <= *duty;
                *phase = (*phase + PSG_BASE_HZ * step / OUTPUT_RATE as f64).fract();
                Some(if high { 0.5 } else { -0.5 })
            }
            Source::Noise { state, phase, value } => {
                *phase += PSG_BASE_HZ * NOISE_SPEED * step / OUTPUT_RATE as f64;
                while *phase >= 1.0 {
                    *phase -= 1.0;
                    let bit = *state & 1;
                    *state >>= 1;
                    if bit != 0 {
                        *state ^= 0x6000;
                    }
                    *value = if bit != 0 { 0.5 } else { -0.5 };
                }
                Some(*value)
            }
        }
    }

    fn update_envelope(&mut self) {
        match self.stage {
            Stage::Attack => {
                self.ampl = self.attack * self.ampl / 255;
                if self.ampl == 0 {
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.ampl -= self.decay;
                if self.ampl <= self.sustain << AMPL_SHIFT {
                    self.ampl = self.sustain << AMPL_SHIFT;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {}
            Stage::Release => self.ampl -= self.release,
        }
    }
}

struct Track {
    at: usize,
    wait: i64,
    calls: Vec<usize>,
    loops: Vec<(usize, u8)>,
    volume: u8,
    expression: u8,
    pan: u8,
    bend: i8,
    bend_range: u8,
    transpose: i8,
    program: usize,
    attack: u8,
    decay: u8,
    sustain: u8,
    release: u8,
    note_wait: bool,
    tie: bool,
    lfo_depth: u8,
    lfo_speed: u8,
    lfo_range: u8,
    lfo_type: u8,
    lfo_delay: u16,
    condition: bool,
    done: bool,
    /// La piste est revenue en arrière : la musique a fait un tour.
    looped: bool,
}

impl Track {
    fn new(at: usize) -> Self {
        Track { at, wait: 0, calls: Vec::new(), loops: Vec::new(), volume: 127, expression: 127, pan: 64, bend: 0, bend_range: 2, transpose: 0, program: 0, attack: 0xFF, decay: 0xFF, sustain: 0xFF, release: 0xFF, note_wait: true, tie: false, lfo_depth: 0, lfo_speed: 16, lfo_range: 1, lfo_type: 0, lfo_delay: 0, condition: true, done: false, looped: false }
    }
}

#[derive(Clone, Copy)]
enum Next {
    Plain,
    Random,
    Variable,
}

struct Player<'a> {
    data: &'a [u8],
    bank: &'a [Instrument],
    archives: &'a [Option<&'a [u8]>; 4],
    waves: HashMap<(u16, u16), Option<Rc<Wave>>>,
    tracks: Vec<Track>,
    voices: Vec<Voice>,
    variables: [i16; 32],
    tempo: i32,
    master: u8,
    sequence_db: i32,
    random: u32,
    next_argument: Next,
}

impl Player<'_> {
    fn byte(&mut self, track: usize) -> u8 {
        let at = self.tracks[track].at;
        self.tracks[track].at += 1;
        match self.data.get(at) {
            Some(byte) => *byte,
            None => {
                self.tracks[track].done = true;
                0xFF
            }
        }
    }

    fn le(&mut self, track: usize, bytes: usize) -> i32 {
        (0..bytes).fold(0, |value, i| value | (self.byte(track) as i32) << (i * 8))
    }

    fn varlen(&mut self, track: usize) -> i32 {
        let mut value = 0;
        for _ in 0..4 {
            let byte = self.byte(track);
            value = value << 7 | (byte & 0x7F) as i32;
            if byte & 0x80 == 0 {
                break;
            }
        }
        value
    }

    fn random(&mut self) -> u32 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 17;
        self.random ^= self.random << 5;
        self.random
    }

    /// Le dernier argument d'une commande : écrit, tiré au sort (0xA0) ou pris dans une variable (0xA1).
    fn last(&mut self, track: usize, read: impl FnOnce(&mut Self, usize) -> i32) -> i32 {
        match std::mem::replace(&mut self.next_argument, Next::Plain) {
            Next::Plain => read(self, track),
            Next::Random => {
                let (min, max) = (self.le(track, 2) as i16 as i32, self.le(track, 2) as i16 as i32);
                let span = (max - min).unsigned_abs() + 1;
                min.min(max) + (self.random() % span) as i32
            }
            Next::Variable => {
                let at = self.byte(track) as usize;
                self.variables.get(at).copied().unwrap_or(0) as i32
            }
        }
    }

    fn last_u8(&mut self, track: usize) -> u8 {
        self.last(track, |p, t| p.byte(t) as i32) as u8
    }

    fn last_s16(&mut self, track: usize) -> i32 {
        self.last(track, |p, t| p.le(t, 2) as i16 as i32)
    }

    fn last_varlen(&mut self, track: usize) -> i32 {
        self.last(track, |p, t| p.varlen(t))
    }

    fn wave(&mut self, region: &Region) -> Option<Rc<Wave>> {
        let archives = self.archives;
        self.waves.entry((region.archive, region.wave)).or_insert_with(|| swar_wave((*archives.get(region.archive as usize)?)?, region.wave as usize).map(Rc::new)).clone()
    }

    fn play(&mut self, track: usize, key: u8, velocity: u8, duration: i32) {
        let t = &self.tracks[track];
        let key = (key as i32 + t.transpose as i32).clamp(0, 127) as u8;
        if t.tie {
            if let Some(tied) = self.voices.iter_mut().find(|v| v.track == track && v.tied && v.stage != Stage::Release) {
                tied.key = key;
                tied.velocity_db = volume_db(velocity);
                return;
            }
        }
        let tie = t.tie;
        let override_or = |own: u8, region: u8| if own == 0xFF { region } else { own };
        let (own_attack, own_decay, own_sustain, own_release, lfo_delay) = (t.attack, t.decay, t.sustain, t.release, t.lfo_delay);
        let Some(region) = self.bank.get(t.program).and_then(|instrument| instrument.region(key)) else { return };
        let source = match region.kind {
            PCM => Source::Wave { wave: match self.wave(&region) { Some(wave) => wave, None => return }, position: 0.0 },
            PSG => Source::Square { duty: region.wave, phase: 0.0 },
            NOISE => Source::Noise { state: 0x7FFF, phase: 0.0, value: 0.0 },
            _ => return,
        };
        if self.voices.len() >= MAX_VOICES {
            // La voix la plus proche de s'éteindre laisse sa place
            let quietest = (0..self.voices.len()).min_by_key(|&i| (self.voices[i].stage != Stage::Release, self.voices[i].ampl));
            if let Some(i) = quietest {
                self.voices.swap_remove(i);
            }
        }
        self.voices.push(Voice {
            source,
            track,
            key,
            base_key: region.key,
            velocity_db: volume_db(velocity),
            region_pan: region.pan as i32 - 64,
            stage: Stage::Attack,
            ampl: AMPL_MIN,
            attack: attack_level(override_or(own_attack, region.attack)),
            decay: fall_rate(override_or(own_decay, region.decay)),
            sustain: volume_db(override_or(own_sustain, region.sustain)),
            release: fall_rate(override_or(own_release, region.release)),
            remaining: if duration == 0 { -1 } else { duration as i64 },
            lfo_phase: 0.0,
            lfo_wait: lfo_delay,
            tied: tie,
        });
    }

    fn compare(&mut self, command: u8, variable: usize, value: i32) -> Option<bool> {
        let held = self.variables.get(variable).copied().unwrap_or(0) as i32;
        Some(match command {
            0xB8 => held == value,
            0xB9 => held >= value,
            0xBA => held > value,
            0xBB => held <= value,
            0xBC => held < value,
            0xBD => held != value,
            _ => return None,
        })
    }

    /// Le calcul d'une commande sur une variable (0xB0 à 0xB7).
    fn change_variable(&mut self, command: u8, variable: usize, value: i32) {
        let random = self.random();
        let Some(slot) = self.variables.get_mut(variable) else { return };
        let held = *slot as i32;
        *slot = match command {
            0xB0 => value,
            0xB1 => held.wrapping_add(value),
            0xB2 => held.wrapping_sub(value),
            0xB3 => held.wrapping_mul(value),
            0xB4 if value != 0 => held / value,
            0xB5 => if value >= 0 { held.wrapping_shl(value as u32) } else { held.wrapping_shr(value.unsigned_abs()) },
            0xB6 => (random % (value.unsigned_abs() + 1)) as i32 * value.signum().max(1),
            _ => held,
        } as i16;
    }

    fn step_track(&mut self, track: usize) {
        for _ in 0..MAX_COMMANDS_PER_TICK {
            if self.tracks[track].done || self.tracks[track].wait > 0 {
                return;
            }
            // Une commande après 0xA2 ne s'exécute que si la dernière comparaison était vraie
            let mut run = true;
            let mut command = self.byte(track);
            self.next_argument = Next::Plain;
            loop {
                match command {
                    0xA0 => self.next_argument = Next::Random,
                    0xA1 => self.next_argument = Next::Variable,
                    0xA2 => run = run && self.tracks[track].condition,
                    _ => break,
                }
                command = self.byte(track);
            }
            self.command(track, command, run);
        }
        // Une piste qui tourne sans jamais attendre ne rendrait plus la main
        self.tracks[track].done = true;
    }

    fn command(&mut self, track: usize, command: u8, run: bool) {
        match command {
            0x00..=0x7F => {
                let velocity = self.byte(track);
                let duration = self.last_varlen(track);
                if run {
                    self.play(track, command, velocity, duration);
                    if self.tracks[track].note_wait {
                        self.tracks[track].wait = duration as i64;
                    }
                }
            }
            0x80 => {
                let rest = self.last_varlen(track);
                if run {
                    self.tracks[track].wait = rest as i64;
                }
            }
            0x81 => {
                let program = self.last_varlen(track);
                if run {
                    self.tracks[track].program = program as usize & 0x7FFF;
                }
            }
            0x93 => {
                let number = self.byte(track);
                let at = self.le(track, 3) as usize;
                if run && (number as usize) < MAX_TRACKS && self.tracks.len() < MAX_TRACKS {
                    self.tracks.push(Track::new(at));
                }
            }
            0x94 => {
                let at = self.le(track, 3) as usize;
                if run {
                    let t = &mut self.tracks[track];
                    t.looped |= at <= t.at;
                    t.at = at;
                }
            }
            0x95 => {
                let at = self.le(track, 3) as usize;
                if run {
                    let t = &mut self.tracks[track];
                    t.calls.push(t.at);
                    t.at = at;
                }
            }
            0xFD => {
                if run {
                    let t = &mut self.tracks[track];
                    match t.calls.pop() {
                        Some(at) => t.at = at,
                        None => t.done = true,
                    }
                }
            }
            0xB0..=0xBD => {
                let variable = self.byte(track) as usize;
                let value = self.last_s16(track);
                if run {
                    match self.compare(command, variable, value) {
                        Some(result) => self.tracks[track].condition = result,
                        None => self.change_variable(command, variable, value),
                    }
                }
            }
            0xC0..=0xCF | 0xD0..=0xD6 => {
                let value = self.last_u8(track);
                if run {
                    self.set_u8(track, command, value);
                }
            }
            0xE0 | 0xE1 | 0xE3 => {
                let value = self.last_s16(track);
                if run {
                    match command {
                        0xE0 => self.tracks[track].lfo_delay = value as u16,
                        0xE1 => self.tempo = value,
                        _ => {}
                    }
                }
            }
            0xFC => {
                if run {
                    let t = &mut self.tracks[track];
                    if let Some((start, left)) = t.loops.last_mut() {
                        if *left == 0 {
                            // Boucle sans fin
                            t.looped = true;
                            t.at = *start;
                        } else {
                            *left -= 1;
                            if *left == 0 {
                                t.loops.pop();
                            } else {
                                t.at = *start;
                            }
                        }
                    }
                }
            }
            0xFE => {
                self.le(track, 2);
            }
            // 0xFF : fin ; une commande inconnue laisserait la suite illisible
            _ => {
                if run || command != 0xFF {
                    self.tracks[track].done = true;
                }
            }
        }
    }

    fn set_u8(&mut self, track: usize, command: u8, value: u8) {
        let t = &mut self.tracks[track];
        match command {
            0xC0 => t.pan = value,
            0xC1 => t.volume = value,
            0xC2 => self.master = value,
            0xC3 => t.transpose = value as i8,
            0xC4 => t.bend = value as i8,
            0xC5 => t.bend_range = value,
            0xC7 => t.note_wait = value != 0,
            0xC8 => t.tie = value != 0,
            0xCA => t.lfo_depth = value,
            0xCB => t.lfo_speed = value,
            0xCC => t.lfo_type = value,
            0xCD => t.lfo_range = value,
            0xD0 => t.attack = value,
            0xD1 => t.decay = value,
            0xD2 => t.sustain = value,
            0xD3 => t.release = value,
            0xD4 => t.loops.push((t.at, value)),
            0xD5 => t.expression = value,
            _ => {}
        }
    }

    /// Un pas de séquence : chaque note raccourcit, chaque piste avance.
    fn tick(&mut self) {
        for voice in &mut self.voices {
            if voice.remaining > 0 {
                voice.remaining -= 1;
                if voice.remaining == 0 && voice.stage != Stage::Release {
                    voice.stage = Stage::Release;
                }
            }
        }
        // Une piste peut en ouvrir d'autres : elles démarrent au même pas
        let mut track = 0;
        while track < self.tracks.len() {
            if self.tracks[track].wait > 0 {
                self.tracks[track].wait -= 1;
            }
            self.step_track(track);
            track += 1;
        }
    }

    fn mix(&mut self, out: &mut [f32]) {
        let master = volume_db(self.master) + self.sequence_db;
        let tracks = &self.tracks;
        self.voices.retain_mut(|voice| {
            voice.update_envelope();
            if voice.stage == Stage::Release && voice.ampl <= AMPL_MIN {
                return false;
            }
            let t = &tracks[voice.track];
            let db = (voice.ampl >> AMPL_SHIFT) + voice.velocity_db + volume_db(t.volume) + volume_db(t.expression) + master;
            let gain = if db <= SILENCE { 0.0 } else { 10f32.powf(db as f32 / 200.0) };
            let pan = ((t.pan as i32 + voice.region_pan).clamp(0, 127)) as f32 / 127.0;
            let (left, right) = (gain * (1.0 - pan), gain * pan);
            let mut lfo = 0.0;
            if voice.lfo_wait > 0 {
                voice.lfo_wait -= 1;
            } else if t.lfo_depth > 0 && t.lfo_type == 0 {
                voice.lfo_phase = (voice.lfo_phase + t.lfo_speed as f64 * LFO_STEP).fract();
                lfo = (voice.lfo_phase * std::f64::consts::TAU).sin() * t.lfo_depth as f64 * t.lfo_range as f64 * LFO_SCALE;
            }
            let semitones = voice.key as f64 - voice.base_key as f64 + t.bend as f64 * t.bend_range as f64 / 128.0 + lfo;
            let step = 2f64.powf(semitones / 12.0);
            for frame in out.chunks_exact_mut(2) {
                let Some(sample) = voice.next_sample(step) else { return false };
                frame[0] += sample * left;
                frame[1] += sample * right;
            }
            true
        });
    }
}

/// Joue les commandes à partir de `start` : jusqu'à la fin des pistes ou un tour de la boucle.
pub fn render(data: &[u8], start: usize, bank: &[Instrument], archives: &[Option<&[u8]>; 4], volume: u8) -> Sound {
    let mut player = Player { data, bank, archives, waves: HashMap::new(), tracks: vec![Track::new(start)], voices: Vec::new(), variables: [0; 32], tempo: 120, master: 127, sequence_db: volume_db(volume), random: 0x2545_F491, next_argument: Next::Plain };
    // La première note joue dès le début : les pistes démarrent sans attendre un pas de tempo
    player.step_track(0);
    let per_update = OUTPUT_RATE as f64 * UPDATE_SECONDS;
    let (mut carried, mut tempo_units, mut elapsed, mut tail) = (0.0, 0, 0.0, None);
    let mut mixed: Vec<f32> = Vec::new();
    let mut update = Vec::new();
    while elapsed < MAX_SECONDS {
        tempo_units += player.tempo.max(0);
        while tempo_units >= TEMPO_UNITS {
            tempo_units -= TEMPO_UNITS;
            player.tick();
        }
        let finished = player.tracks.iter().all(|t| t.done || t.looped);
        if finished && tail.is_none() {
            tail = Some(elapsed);
            // Une note en boucle ne s'éteindrait jamais
            for voice in player.voices.iter_mut().filter(|v| v.keeps_playing_forever() && (v.remaining < 0 || v.remaining > 1)) {
                voice.stage = Stage::Release;
                voice.release = voice.release.max(fall_rate(100));
            }
        }
        if tail.is_some_and(|since: f64| player.voices.is_empty() || elapsed - since > MAX_TAIL_SECONDS) {
            break;
        }
        carried += per_update;
        let count = carried as usize;
        carried -= count as f64;
        update.clear();
        update.resize(count * 2, 0.0f32);
        player.mix(&mut update);
        mixed.extend_from_slice(&update);
        elapsed += UPDATE_SECONDS;
    }
    Sound { rate: OUTPUT_RATE, channels: 2, samples: mixed.iter().map(|s| ((s * MIX_LEVEL).clamp(-1.0, 1.0) * 32767.0) as i16).collect() }
}

/// Une note de l'instrument `index` (une seconde au tempo de départ).
pub fn render_instrument(bank: &[Instrument], index: usize, archives: &[Option<&[u8]>; 4]) -> Option<Sound> {
    const ONE_SECOND: u8 = 96;
    let key = bank.get(index)?.sample_key();
    let mut commands = vec![0x81];
    if index >= 0x80 {
        commands.push(0x80 | (index >> 7) as u8);
    }
    commands.extend([(index & 0x7F) as u8, key, 127, ONE_SECOND, 0xFF]);
    Some(render(&commands, 0, bank, archives, 127))
}
