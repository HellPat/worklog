//! Deterministic speech fixtures; no devices, windows, or playback.
use anyhow::{Result, ensure};
use team_up::echo::{Cleaner, FRAME, RATE};
fn speech(path: &str) -> Result<Vec<f32>> {
    let mut wav = hound::WavReader::open(path)?;
    let spec = wav.spec();
    let raw: Vec<f32> = wav
        .samples::<i16>()
        .map(|s| s.map(|s| f32::from(s) / 32768.))
        .collect::<std::result::Result<_, _>>()?;
    let mut resampler =
        team_up::audio::Resampler::new(spec.sample_rate, usize::from(spec.channels));
    Ok(resampler
        .convert(&raw)
        .chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.)
        .collect())
}
fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / samples.len().max(1) as f64
}
fn alignment(actual: &[f32], expected: &[f32]) -> (f64, f64) {
    let best = (-800_i32..=800)
        .map(|delay| {
            let (actual, expected) = if delay >= 0 {
                (&actual[delay as usize..], expected)
            } else {
                (actual, &expected[(-delay) as usize..])
            };
            let (mut aa, mut bb, mut ab) = (0., 0., 0.);
            for (a, b) in actual.iter().zip(expected.iter()) {
                let (a, b) = (f64::from(*a), f64::from(*b));
                aa += a * a;
                bb += b * b;
                ab += a * b;
            }
            (ab / (aa * bb).sqrt().max(1e-12), ab / bb.max(1e-12), delay)
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .unwrap_or_default();
    println!("best alignment {} samples", best.2);
    (best.0, best.1)
}

fn simulated_echo(delay: usize, gain: f32) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>)> {
    let far = speech("fixtures/asr/spoken.wav")?;
    let near = speech("fixtures/asr/live-speech.wav")?;
    let frames = 40 * 100;
    let mut cleaner = Cleaner::default();
    let mut mic = Vec::new();
    let mut clean = Vec::new();
    let mut local = Vec::new();
    for block in 0..frames {
        let mut reference = [0.; FRAME];
        let mut microphone = [0.; FRAME];
        for i in 0..FRAME {
            let t = block * FRAME + i;
            let r = far[t % far.len()];
            let n = if t >= 20 * RATE {
                0.65 * near[(t - 20 * RATE) % near.len()]
            } else {
                0.
            };
            let echo = if t >= delay {
                gain * far[(t - delay) % far.len()]
            } else {
                0.
            };
            let reflection = if t >= delay + 320 {
                0.15 * far[(t - delay - 320) % far.len()]
            } else {
                0.
            };
            reference[i] = r;
            microphone[i] = n + echo + reflection;
            local.push(n);
        }
        let output = cleaner.process_frame(&reference, &microphone)?;
        mic.extend(microphone);
        clean.extend(output);
    }
    Ok((mic, clean, local))
}
#[test]
fn sonora_cancels_delayed_speaker_speech_and_preserves_double_talk() -> Result<()> {
    for (delay, echo_gain) in [(RATE / 25, 0.35), (RATE / 10, 0.65)] {
        let (mic, clean, local) = simulated_echo(delay, echo_gain)?;
        let suppression = 10.
            * (energy(&mic[5 * RATE..19 * RATE]) / energy(&clean[5 * RATE..19 * RATE]).max(1e-15))
                .log10();
        let (correlation, gain) = alignment(&clean[22 * RATE..], &local[22 * RATE..]);
        println!(
            "Sonora (delay={delay} samples, echo gain={echo_gain}): echo reduction {suppression:.1} dB; double-talk correlation {correlation:.3}; voice gain {gain:.3}"
        );
        ensure!(
            suppression >= 15.,
            "Echo-only speech was not sufficiently removed: {suppression:.1} dB"
        );
        ensure!(
            correlation >= 0.8 && (0.5..1.5).contains(&gain),
            "Local voice damaged during overlap: correlation={correlation}, gain={gain}"
        );
    }
    Ok(())
}
#[test]
fn sonora_preserves_microphone_speech_without_playback() -> Result<()> {
    let near = speech("fixtures/asr/live-speech.wav")?;
    let mut cleaner = Cleaner::default();
    let mut input = Vec::new();
    let mut output = Vec::new();
    for block in 0..1200 {
        let mut microphone = [0.; FRAME];
        for (i, sample) in microphone.iter_mut().enumerate() {
            *sample = near[(block * FRAME + i) % near.len()];
        }
        input.extend(microphone);
        output.extend(cleaner.process_frame(&[0.; FRAME], &microphone)?);
    }
    let (correlation, gain) = alignment(&output[RATE..], &input[RATE..]);
    println!("Sonora mic-only: correlation {correlation:.3}, gain {gain:.3}");
    ensure!(
        correlation >= 0.95 && (0.8..1.2).contains(&gain),
        "Mic-only voice changed unexpectedly"
    );
    Ok(())
}

#[test]
#[ignore = "runs the installed local ASR on speech fixtures only; no microphone or playback"]
fn cleaned_double_talk_remains_transcribable_and_echo_only_is_silent() -> Result<()> {
    use std::{path::Path, sync::mpsc, thread};
    use team_up::audio::{AudioEvent, Runtime, stream_wav};
    fn save(path: &Path, samples: &[f32]) -> Result<()> {
        let mut wav = hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels: 1,
                sample_rate: RATE as u32,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        for sample in samples {
            wav.write_sample((sample.clamp(-1., 1.) * 32767.) as i16)?;
        }
        wav.finalize()?;
        Ok(())
    }
    fn words(text: &str) -> Vec<String> {
        text.split_whitespace()
            .map(|s| {
                s.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .filter(|s| !s.is_empty())
            .collect()
    }
    fn distance(a: &[String], b: &[String]) -> usize {
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, x) in a.iter().enumerate() {
            let mut previous = row[0];
            row[0] = i + 1;
            for (j, y) in b.iter().enumerate() {
                let old = row[j + 1];
                row[j + 1] = (row[j] + 1)
                    .min(old + 1)
                    .min(previous + usize::from(x != y));
                previous = old;
            }
        }
        row[b.len()]
    }
    let (_, clean, local) = simulated_echo(RATE / 10, 0.65)?;
    let dir = tempfile::tempdir()?;
    let baseline = dir.path().join("local.wav");
    let cleaned = dir.path().join("clean.wav");
    let echo = dir.path().join("echo-only.wav");
    save(&baseline, &local[22 * RATE..35 * RATE])?;
    save(&cleaned, &clean[22 * RATE + 128..35 * RATE + 128])?;
    save(&echo, &clean[5 * RATE..15 * RATE])?;
    let runtime = Runtime::start()?;
    let port = runtime.port;
    let outputs = thread::scope(|scope| -> Result<Vec<String>> {
        let workers: Vec<_> = [baseline, cleaned, echo]
            .into_iter()
            .map(|path| {
                scope.spawn(move || -> Result<String> {
                    let (tx, rx) = mpsc::channel();
                    stream_wav(port, &path, tx)?;
                    let mut result = vec![];
                    for event in rx.try_iter() {
                        match event {
                            AudioEvent::Transcript(segment) => result.push(segment.text),
                            AudioEvent::Error(error) => anyhow::bail!(error),
                            _ => {}
                        }
                    }
                    Ok(result.join(" "))
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| {
                worker
                    .join()
                    .map_err(|_| anyhow::anyhow!("ASR fixture worker panicked"))?
            })
            .collect()
    })?;
    println!(
        "Baseline: {}\nCleaned: {}\nEcho-only: {}",
        outputs[0], outputs[1], outputs[2]
    );
    let baseline = words(&outputs[0]);
    let cleaned = words(&outputs[1]);
    ensure!(
        baseline.len() >= 8,
        "Baseline fixture produced insufficient speech"
    );
    let wer = distance(&baseline, &cleaned) as f64 / baseline.len() as f64;
    println!(
        "Cleaned transcript difference from isolated voice: {:.1}%",
        wer * 100.
    );
    ensure!(
        wer <= 0.25,
        "Echo cleanup changed too much local speech: WER={wer}"
    );
    ensure!(
        outputs[2].trim().is_empty(),
        "Echo-only microphone produced a duplicate transcript"
    );
    Ok(())
}
