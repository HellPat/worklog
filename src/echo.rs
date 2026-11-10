//! Local playback-reference echo cancellation. Raw recordings bypass this stage.
use anyhow::{Context, Result};
use sonora::{AudioProcessing, Config, StreamConfig, config::EchoCanceller};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Condvar, Mutex},
    time::{Duration, Instant},
};
pub const RATE: usize = 16_000;
pub const FRAME: usize = RATE / 100;

#[derive(Default)]
struct RenderHistory {
    chunks: BTreeMap<i64, Vec<f32>>,
    last: Option<Instant>,
}
pub struct Reference {
    epoch: Instant,
    history: Mutex<RenderHistory>,
    changed: Condvar,
}
impl Default for Reference {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            history: Mutex::default(),
            changed: Condvar::new(),
        }
    }
}
impl Reference {
    pub fn position(&self, instant: Instant) -> i64 {
        if instant >= self.epoch {
            (instant.duration_since(self.epoch).as_secs_f64() * RATE as f64) as i64
        } else {
            -(self.epoch.duration_since(instant).as_secs_f64() * RATE as f64) as i64
        }
    }
    pub fn push(&self, start: i64, samples: Vec<f32>) -> Result<()> {
        let mut history = self
            .history
            .lock()
            .map_err(|_| anyhow::anyhow!("Echo reference lock failed"))?;
        history.chunks.insert(start, samples);
        history.last = Some(Instant::now());
        let cutoff = start - 3 * RATE as i64;
        while history
            .chunks
            .first_key_value()
            .is_some_and(|(time, data)| *time + (data.len() as i64) < cutoff)
        {
            history.chunks.pop_first();
        }
        self.changed.notify_all();
        Ok(())
    }
    pub fn frame(&self, start: i64) -> Result<[f32; FRAME]> {
        let mut history = self
            .history
            .lock()
            .map_err(|_| anyhow::anyhow!("Echo reference lock failed"))?;
        let end = start + FRAME as i64;
        // Capture and loopback callbacks can arrive in either order. Wait only
        // for a recent render stream, never for output that is silent/inactive.
        let deadline = Instant::now() + Duration::from_millis(30);
        while history
            .last
            .is_some_and(|last| last.elapsed() < Duration::from_millis(100))
            && history
                .chunks
                .last_key_value()
                .is_some_and(|(time, data)| *time + (data.len() as i64) < end)
        {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            let (updated, timeout) = self
                .changed
                .wait_timeout(history, remaining)
                .map_err(|_| anyhow::anyhow!("Echo reference wait failed"))?;
            history = updated;
            if timeout.timed_out() {
                break;
            }
        }
        let mut result = [0.; FRAME];
        for (time, samples) in history.chunks.range(..end).rev() {
            let chunk_end = *time + samples.len() as i64;
            if chunk_end <= start {
                break;
            }
            let from = (*time).max(start);
            let to = chunk_end.min(end);
            if from < to {
                result[(from - start) as usize..(to - start) as usize]
                    .copy_from_slice(&samples[(from - time) as usize..(to - time) as usize]);
            }
        }
        Ok(result)
    }
}

pub struct Cleaner {
    apm: AudioProcessing,
    pending: VecDeque<f32>,
    next: Option<i64>,
}
impl Default for Cleaner {
    fn default() -> Self {
        // This recorder must retain local speech during simultaneous playback.
        // Favor transparency over aggressive residual suppression; keep noise
        // suppression and automatic gain control disabled.
        let mut aec = sonora::config::EchoCanceller3Config::default();
        aec.suppressor.dominant_nearend_detection.enr_threshold = 2.;
        aec.suppressor.dominant_nearend_detection.snr_threshold = 10.;
        aec.suppressor.dominant_nearend_detection.trigger_threshold = 4;
        aec.suppressor.normal_tuning.mask_lf.enr_transparent = 1.;
        aec.suppressor.normal_tuning.mask_lf.enr_suppress = 2.;
        aec.suppressor.normal_tuning.mask_hf.enr_transparent = 1.;
        aec.suppressor.normal_tuning.mask_hf.enr_suppress = 2.;
        aec.suppressor.nearend_tuning.mask_lf.enr_transparent = 20.;
        aec.suppressor.nearend_tuning.mask_lf.enr_suppress = 30.;
        aec.suppressor.nearend_tuning.mask_hf.enr_transparent = 20.;
        aec.suppressor.nearend_tuning.mask_hf.enr_suppress = 30.;
        Self {
            apm: AudioProcessing::builder()
                .echo_canceller3_config(aec)
                .config(Config {
                    echo_canceller: Some(EchoCanceller {
                        enforce_high_pass_filtering: false,
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .capture_config(StreamConfig::new(RATE as u32, 1))
                .render_config(StreamConfig::new(RATE as u32, 1))
                .build(),
            pending: VecDeque::new(),
            next: None,
        }
    }
}
impl Cleaner {
    pub fn process_frame(
        &mut self,
        render: &[f32; FRAME],
        microphone: &[f32; FRAME],
    ) -> Result<[f32; FRAME]> {
        let mut render_out = [0.; FRAME];
        let mut clean = [0.; FRAME];
        self.apm
            .process_render_f32(&[render], &mut [&mut render_out])
            .context("Sonora playback processing failed")?;
        self.apm
            .set_stream_delay_ms(0)
            .context("Sonora stream timing failed")?;
        self.apm
            .process_capture_f32(&[microphone], &mut [&mut clean])
            .context("Sonora microphone processing failed")?;
        Ok(clean)
    }
    pub fn push(&mut self, start: i64, samples: &[f32], reference: &Reference) -> Result<Vec<f32>> {
        // Preserve continuous sample timing through differently sized device
        // callbacks. A real device discontinuity starts a new filter history.
        if self
            .next
            .is_some_and(|next| (start - next - self.pending.len() as i64).abs() > RATE as i64 / 10)
        {
            *self = Self::default();
        }
        if self.next.is_none() {
            self.next = Some(start);
        }
        self.pending.extend(samples);
        let mut output = Vec::with_capacity(samples.len() + FRAME);
        while self.pending.len() >= FRAME {
            let time = self.next.unwrap_or(start);
            let mut microphone = [0.; FRAME];
            for sample in &mut microphone {
                *sample = self.pending.pop_front().unwrap_or(0.);
            }
            output.extend(self.process_frame(&reference.frame(time)?, &microphone)?);
            self.next = Some(time + FRAME as i64);
        }
        Ok(output)
    }
    pub fn finish(&mut self, reference: &Reference) -> Result<Vec<f32>> {
        // AEC3's frame/block conversion delays capture by 128 samples at 16 kHz.
        // Flush that latency as well as an incomplete final device frame.
        let wanted = self.pending.len() + 128;
        let time = self.next.unwrap_or(0);
        let padding = (wanted.div_ceil(FRAME) * FRAME) - self.pending.len();
        let start = time + self.pending.len() as i64;
        let mut output = self.push(start, &vec![0.; padding], reference)?;
        output.truncate(wanted);
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reference_preserves_timestamps_and_leaves_silent_gaps_empty() -> Result<()> {
        let reference = Reference::default();
        reference.push(0, vec![0.25; FRAME])?;
        reference.push((FRAME * 2) as i64, vec![0.75; FRAME])?;
        assert_eq!(reference.frame(0)?, [0.25; FRAME]);
        assert_eq!(reference.frame(FRAME as i64)?, [0.; FRAME]);
        assert_eq!(reference.frame((FRAME * 2) as i64)?, [0.75; FRAME]);
        // More than three seconds later must not replay old speaker audio.
        reference.push((RATE * 4) as i64, vec![0.5; FRAME])?;
        assert_eq!(reference.frame(0)?, [0.; FRAME]);
        Ok(())
    }
    #[test]
    fn partial_device_frames_flush_without_losing_tail_speech() -> Result<()> {
        let reference = Reference::default();
        let mut cleaner = Cleaner::default();
        let mut input = vec![0.; FRAME * 7 + 41];
        input[FRAME * 7 + 30] = 0.5;
        let mut output = vec![];
        let mut position = 0;
        for block in input.chunks(73) {
            output.extend(cleaner.push(position, block, &reference)?);
            position += block.len() as i64;
        }
        output.extend(cleaner.finish(&reference)?);
        assert_eq!(output.len(), input.len() + 128);
        assert!(
            (output[FRAME * 7 + 30 + 128] - 0.5).abs() < 0.01,
            "Tail signal did not survive echo frame buffering"
        );
        Ok(())
    }
}
