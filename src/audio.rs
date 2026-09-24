//! Native game audio. File I/O and decoding are off the JS/event-loop thread;
//! Kira owns the realtime output callback and receives only control commands.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use deno_core::{OpState, op2};
use deno_error::JsErrorBox;
use kira::sound::PlaybackState;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::track::MainTrackBuilder;
use kira::{AudioManager, AudioManagerSettings, Decibels, DefaultBackend, Tween};
use serde::Serialize;
use symphonia::core::audio::{
    Audio, AudioBuffer, GenericAudioBufferRef,
    conv::{FromSample, IntoSample},
    sample::Sample,
};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::formats::TrackType;
use symphonia::core::io::MediaSourceStream;

use crate::host::{SharedHost, read_asset_limited};

const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_DECODED_BYTES: usize = 128 * 1024 * 1024;
const MAX_CLIPS: usize = 128;
const MAX_VOICES: usize = 64;
const MAX_FRAMES: usize = MAX_DECODED_BYTES / std::mem::size_of::<kira::Frame>();
static AUDIO_DECODE_LIMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

#[derive(Default)]
struct AudioService {
    manager: Option<AudioManager<DefaultBackend>>,
    clips: HashMap<u32, Clip>,
    voices: HashMap<u32, StaticSoundHandle>,
    decoded_bytes: usize,
    next_clip_id: u32,
    next_voice_id: u32,
}

struct Clip {
    sound: StaticSoundData,
    bytes: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioClipInfo {
    id: u32,
    duration_seconds: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioVoiceInfo {
    state: &'static str,
    position_seconds: f64,
}

impl AudioService {
    fn manager(&mut self) -> Result<&mut AudioManager<DefaultBackend>, JsErrorBox> {
        if self.manager.is_none() {
            let settings = AudioManagerSettings {
                main_track_builder: MainTrackBuilder::new().sound_capacity(MAX_VOICES),
                ..Default::default()
            };
            self.manager = Some(AudioManager::<DefaultBackend>::new(settings).map_err(
                |error| JsErrorBox::generic(format!("cannot initialize audio output: {error}")),
            )?);
        }
        Ok(self
            .manager
            .as_mut()
            .expect("audio manager was initialized"))
    }

    fn allocate_clip_id(&mut self) -> Result<u32, JsErrorBox> {
        let clips = &self.clips;
        allocate_id(&mut self.next_clip_id, |id| clips.contains_key(&id))
    }

    fn allocate_voice_id(&mut self) -> Result<u32, JsErrorBox> {
        let voices = &self.voices;
        allocate_id(&mut self.next_voice_id, |id| voices.contains_key(&id))
    }

    fn insert_clip(&mut self, sound: StaticSoundData) -> Result<AudioClipInfo, JsErrorBox> {
        if self.clips.len() >= MAX_CLIPS {
            return Err(JsErrorBox::range_error("audio clip limit reached"));
        }
        let bytes = sound
            .frames
            .len()
            .checked_mul(std::mem::size_of::<kira::Frame>())
            .ok_or_else(|| JsErrorBox::range_error("decoded audio is too large"))?;
        let total = self
            .decoded_bytes
            .checked_add(bytes)
            .ok_or_else(|| JsErrorBox::range_error("decoded audio is too large"))?;
        if total > MAX_DECODED_BYTES {
            return Err(JsErrorBox::range_error(
                "decoded audio cache exceeds 128 MiB limit",
            ));
        }
        let id = self.allocate_clip_id()?;
        let duration_seconds = sound.duration().as_secs_f64();
        self.clips.insert(id, Clip { sound, bytes });
        self.decoded_bytes = total;
        Ok(AudioClipInfo {
            id,
            duration_seconds,
        })
    }

    fn play(&mut self, clip_id: u32, volume: f64, looped: bool) -> Result<u32, JsErrorBox> {
        let volume = volume_decibels(volume)?;
        self.voices
            .retain(|_, voice| voice.state() != PlaybackState::Stopped);
        if self.voices.len() >= MAX_VOICES {
            return Err(JsErrorBox::range_error("audio voice limit reached"));
        }
        let clip = self
            .clips
            .get(&clip_id)
            .ok_or_else(|| JsErrorBox::type_error("unknown audio clip"))?;
        let mut sound = clip.sound.volume(volume);
        if looped {
            sound = sound.loop_region(0.0..);
        }
        let id = self.allocate_voice_id()?;
        let voice = self
            .manager()?
            .play(sound)
            .map_err(|error| JsErrorBox::generic(format!("cannot play audio: {error}")))?;
        self.voices.insert(id, voice);
        Ok(id)
    }

    fn voice(&mut self, id: u32) -> Result<&mut StaticSoundHandle, JsErrorBox> {
        self.voices
            .get_mut(&id)
            .ok_or_else(|| JsErrorBox::type_error("unknown audio voice"))
    }

    fn unload(&mut self, id: u32) -> Result<(), JsErrorBox> {
        let clip = self
            .clips
            .remove(&id)
            .ok_or_else(|| JsErrorBox::type_error("unknown audio clip"))?;
        self.decoded_bytes -= clip.bytes;
        Ok(())
    }
}

fn allocate_id(next: &mut u32, mut occupied: impl FnMut(u32) -> bool) -> Result<u32, JsErrorBox> {
    for _ in 0..u32::MAX {
        *next = next.wrapping_add(1).max(1);
        if !occupied(*next) {
            return Ok(*next);
        }
    }
    Err(JsErrorBox::range_error("audio handle space exhausted"))
}

fn volume_decibels(volume: f64) -> Result<Decibels, JsErrorBox> {
    if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
        return Err(JsErrorBox::range_error(
            "audio volume must be between 0 and 1",
        ));
    }
    if volume == 0.0 {
        Ok(Decibels::SILENCE)
    } else {
        Ok(Decibels((20.0 * volume.log10()) as f32))
    }
}

/// Kira's convenience decoder grows the whole clip before returning. Decode
/// packet by packet here so an unexpectedly long compressed file cannot grow
/// beyond the advertised 128 MiB cache limit before we reject it.
fn decode_bounded(bytes: Vec<u8>) -> Result<StaticSoundData, JsErrorBox> {
    decode_bounded_with_limit(bytes, MAX_FRAMES)
}

fn decode_bounded_with_limit(
    bytes: Vec<u8>,
    max_frames: usize,
) -> Result<StaticSoundData, JsErrorBox> {
    let codecs = symphonia::default::get_codecs();
    let probe = symphonia::default::get_probe();
    let stream = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    let mut format = probe
        .probe(
            &Default::default(),
            stream,
            Default::default(),
            Default::default(),
        )
        .map_err(|error| JsErrorBox::generic(format!("cannot identify audio: {error}")))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| JsErrorBox::generic("audio file has no default track"))?;
    let track_id = track.id;
    let params = match track.codec_params.as_ref() {
        Some(CodecParameters::Audio(params)) => params,
        _ => return Err(JsErrorBox::generic("audio track has no codec parameters")),
    };
    let sample_rate = params
        .sample_rate
        .ok_or_else(|| JsErrorBox::generic("audio sample rate is unknown"))?;
    let mut decoder = codecs
        .make_audio_decoder(params, &Default::default())
        .map_err(|error| JsErrorBox::generic(format!("unsupported audio codec: {error}")))?;
    let mut frames = Vec::new();
    loop {
        let packet = format
            .next_packet()
            .map_err(|error| JsErrorBox::generic(format!("cannot read audio packet: {error}")))?;
        let Some(packet) = packet else { break };
        if packet.track_id != track_id {
            continue;
        }
        let buffer = decoder
            .decode(&packet)
            .map_err(|error| JsErrorBox::generic(format!("cannot decode audio: {error}")))?;
        let new_len = frames
            .len()
            .checked_add(buffer.frames())
            .ok_or_else(|| JsErrorBox::range_error("decoded audio exceeds 128 MiB limit"))?;
        if new_len > max_frames {
            return Err(JsErrorBox::range_error(
                "decoded audio exceeds 128 MiB limit",
            ));
        }
        frames
            .try_reserve_exact(buffer.frames())
            .map_err(|error| JsErrorBox::range_error(format!("cannot allocate audio: {error}")))?;
        append_frames(&mut frames, &buffer)?;
    }
    Ok(StaticSoundData {
        sample_rate,
        frames: Arc::from(frames),
        settings: Default::default(),
        slice: None,
    })
}

fn append_frames(
    frames: &mut Vec<kira::Frame>,
    buffer: &GenericAudioBufferRef<'_>,
) -> Result<(), JsErrorBox> {
    match buffer {
        GenericAudioBufferRef::U8(buffer) => append_typed(frames, buffer),
        GenericAudioBufferRef::U16(buffer) => append_typed(frames, buffer),
        GenericAudioBufferRef::U24(buffer) => append_typed(frames, buffer),
        GenericAudioBufferRef::U32(buffer) => append_typed(frames, buffer),
        GenericAudioBufferRef::S8(buffer) => append_typed(frames, buffer),
        GenericAudioBufferRef::S16(buffer) => append_typed(frames, buffer),
        GenericAudioBufferRef::S24(buffer) => append_typed(frames, buffer),
        GenericAudioBufferRef::S32(buffer) => append_typed(frames, buffer),
        GenericAudioBufferRef::F32(buffer) => append_typed(frames, buffer),
        GenericAudioBufferRef::F64(buffer) => append_typed(frames, buffer),
    }
}

fn append_typed<S: Sample>(
    frames: &mut Vec<kira::Frame>,
    buffer: &AudioBuffer<S>,
) -> Result<(), JsErrorBox>
where
    f32: FromSample<S>,
{
    match buffer.num_planes() {
        1 => {
            let mono = buffer.plane(0).expect("one audio plane was reported");
            frames.extend(
                mono.iter()
                    .map(|sample| kira::Frame::from_mono((*sample).into_sample())),
            );
            Ok(())
        }
        2 => {
            let left = buffer.plane(0).expect("first audio plane was reported");
            let right = buffer.plane(1).expect("second audio plane was reported");
            frames.extend(left.iter().zip(right.iter()).map(|(left, right)| {
                kira::Frame::new((*left).into_sample(), (*right).into_sample())
            }));
            Ok(())
        }
        _ => Err(JsErrorBox::generic("audio must be mono or stereo")),
    }
}

/// A loaded clip is retained in a bounded cache and can be played many times.
/// The audio device is opened only on the first play, so headless asset work
/// does not fail merely because an output device is unavailable.
#[op2]
#[serde]
async fn op_peregrust_audio_load(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<AudioClipInfo, JsErrorBox> {
    let root = state.borrow().borrow::<SharedHost>().asset_root();
    let bytes = read_asset_limited(root, path, MAX_FILE_BYTES as u64).await?;
    let permit = AUDIO_DECODE_LIMIT
        .acquire()
        .await
        .map_err(|error| JsErrorBox::generic(format!("audio decoder unavailable: {error}")))?;
    let sound = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        decode_bounded(bytes)
    })
    .await
    .map_err(|error| JsErrorBox::generic(format!("audio decoder task failed: {error}")))??;
    let mut state = state.borrow_mut();
    state.borrow_mut::<AudioService>().insert_clip(sound)
}

#[op2(fast)]
fn op_peregrust_audio_play(
    state: &mut OpState,
    clip_id: u32,
    volume: f64,
    looped: bool,
) -> Result<u32, JsErrorBox> {
    state
        .borrow_mut::<AudioService>()
        .play(clip_id, volume, looped)
}

#[op2(fast)]
fn op_peregrust_audio_pause(state: &mut OpState, id: u32) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<AudioService>()
        .voice(id)?
        .pause(Tween::default());
    Ok(())
}

#[op2(fast)]
fn op_peregrust_audio_resume(state: &mut OpState, id: u32) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<AudioService>()
        .voice(id)?
        .resume(Tween::default());
    Ok(())
}

#[op2(fast)]
fn op_peregrust_audio_stop(state: &mut OpState, id: u32) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<AudioService>()
        .voice(id)?
        .stop(Tween::default());
    Ok(())
}

#[op2(fast)]
fn op_peregrust_audio_set_volume(
    state: &mut OpState,
    id: u32,
    volume: f64,
) -> Result<(), JsErrorBox> {
    let decibels = volume_decibels(volume)?;
    state
        .borrow_mut::<AudioService>()
        .voice(id)?
        .set_volume(decibels, Tween::default());
    Ok(())
}

#[op2(fast)]
fn op_peregrust_audio_set_loop(
    state: &mut OpState,
    id: u32,
    looped: bool,
) -> Result<(), JsErrorBox> {
    let voice = state.borrow_mut::<AudioService>().voice(id)?;
    if looped {
        voice.set_loop_region(0.0..);
    } else {
        voice.set_loop_region(None);
    }
    Ok(())
}

#[op2]
#[serde]
fn op_peregrust_audio_voice_info(
    state: &mut OpState,
    id: u32,
) -> Result<AudioVoiceInfo, JsErrorBox> {
    let voice = state.borrow_mut::<AudioService>().voice(id)?;
    let state = match voice.state() {
        PlaybackState::Playing => "playing",
        PlaybackState::Pausing => "pausing",
        PlaybackState::Paused => "paused",
        PlaybackState::WaitingToResume => "waitingToResume",
        PlaybackState::Resuming => "resuming",
        PlaybackState::Stopping => "stopping",
        PlaybackState::Stopped => "stopped",
    };
    Ok(AudioVoiceInfo {
        state,
        position_seconds: voice.position(),
    })
}

#[op2(fast)]
fn op_peregrust_audio_dispose_voice(state: &mut OpState, id: u32) -> Result<(), JsErrorBox> {
    let mut voice = state
        .borrow_mut::<AudioService>()
        .voices
        .remove(&id)
        .ok_or_else(|| JsErrorBox::type_error("unknown audio voice"))?;
    voice.stop(Tween::default());
    Ok(())
}

#[op2(fast)]
fn op_peregrust_audio_unload(state: &mut OpState, id: u32) -> Result<(), JsErrorBox> {
    state.borrow_mut::<AudioService>().unload(id)
}

deno_core::extension!(
    peregrust_audio,
    ops = [
        op_peregrust_audio_load,
        op_peregrust_audio_play,
        op_peregrust_audio_pause,
        op_peregrust_audio_resume,
        op_peregrust_audio_stop,
        op_peregrust_audio_set_volume,
        op_peregrust_audio_set_loop,
        op_peregrust_audio_voice_info,
        op_peregrust_audio_dispose_voice,
        op_peregrust_audio_unload,
    ],
    state = |state| state.put(AudioService::default()),
);

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_wav() -> Vec<u8> {
        let samples = [0i16, 1000, -1000, 0];
        let data_len = (samples.len() * 2) as u32;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&16000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        wav
    }

    #[test]
    fn decode_and_cache_without_output_device() {
        let sound = decode_bounded(tiny_wav()).unwrap();
        assert_eq!(sound.sample_rate, 8000);
        assert_eq!(sound.num_frames(), 4);
        let mut service = AudioService::default();
        let clip = service.insert_clip(sound).unwrap();
        assert_eq!(clip.id, 1);
        assert!(clip.duration_seconds > 0.0);
        assert!(service.manager.is_none());
        service.unload(clip.id).unwrap();
        assert_eq!(service.decoded_bytes, 0);
    }

    #[test]
    fn volume_and_handle_validation() {
        assert_eq!(volume_decibels(0.0).unwrap(), Decibels::SILENCE);
        assert_eq!(volume_decibels(1.0).unwrap(), Decibels::IDENTITY);
        assert!(volume_decibels(-0.1).is_err());
        assert!(volume_decibels(f64::NAN).is_err());
        let mut service = AudioService::default();
        assert!(service.play(999, 1.0, false).is_err());
        assert!(service.manager.is_none());
    }

    #[test]
    fn rejects_audio_before_growing_past_frame_budget() {
        assert!(decode_bounded_with_limit(tiny_wav(), 2).is_err());
    }
}
