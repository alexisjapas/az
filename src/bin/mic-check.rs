use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

fn main() -> anyhow::Result<()> {
    let host = cpal::default_host();
    println!("Host audio: {}", host.id().name());

    println!("\n-- Périphériques d'entrée --");
    match host.input_devices() {
        Ok(devices) => {
            let mut any = false;
            for (i, d) in devices.enumerate() {
                any = true;
                let name = d.name().unwrap_or_else(|_| "?".into());
                println!("  [{i}] {name}");
                if let Ok(cfg) = d.default_input_config() {
                    println!(
                        "       default: {} Hz, {} canal(aux), {:?}",
                        cfg.sample_rate().0,
                        cfg.channels(),
                        cfg.sample_format()
                    );
                }
            }
            if !any {
                println!("  (aucun)");
            }
        }
        Err(e) => println!("  erreur d'énumération: {e}"),
    }

    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("aucun périphérique d'entrée par défaut"))?;
    let default_name = device.name().unwrap_or_else(|_| "?".into());
    let config = device.default_input_config()?;
    let stream_config = config.config();
    let sample_format = config.sample_format();

    println!(
        "\nUtilisation par défaut: {default_name}\nFormat: {} Hz, {} canal(aux), {:?}",
        config.sample_rate().0,
        config.channels(),
        sample_format
    );

    let rms_bits = Arc::new(AtomicU32::new(0));
    let peak_bits = Arc::new(AtomicU32::new(0));
    let sample_count = Arc::new(AtomicU32::new(0));

    let err_fn = |e| eprintln!("[stream] erreur: {e}");

    macro_rules! handler {
        ($ty:ty, $to_f32:expr) => {{
            let rms_bits = rms_bits.clone();
            let peak_bits = peak_bits.clone();
            let count = sample_count.clone();
            move |data: &[$ty], _: &_| {
                if data.is_empty() {
                    return;
                }
                let mut sumsq = 0.0f64;
                let mut peak = 0.0f32;
                for &s in data {
                    let v: f32 = $to_f32(s);
                    sumsq += (v as f64) * (v as f64);
                    let a = v.abs();
                    if a > peak {
                        peak = a;
                    }
                }
                let rms = (sumsq / data.len() as f64).sqrt() as f32;
                rms_bits.store(rms.to_bits(), Ordering::Relaxed);
                peak_bits.store(peak.to_bits(), Ordering::Relaxed);
                count.fetch_add(data.len() as u32, Ordering::Relaxed);
            }
        }};
    }

    let stream = match sample_format {
        SampleFormat::F32 => {
            device.build_input_stream(&stream_config, handler!(f32, |s| s), err_fn, None)?
        }
        SampleFormat::I16 => device.build_input_stream(
            &stream_config,
            handler!(i16, |s: i16| s as f32 / i16::MAX as f32),
            err_fn,
            None,
        )?,
        SampleFormat::U16 => device.build_input_stream(
            &stream_config,
            handler!(u16, |s: u16| (s as f32 - 32768.0) / 32768.0),
            err_fn,
            None,
        )?,
        other => anyhow::bail!("format d'échantillon non supporté: {other:?}"),
    };

    stream.play()?;

    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    println!(
        "\nParlez pendant {seconds} s — la barre doit bouger quand vous parlez (silence ~0.001, voix ~0.05+):\n"
    );

    let start = Instant::now();
    let mut last_count = 0u32;
    let mut stale_ticks = 0u32;
    while start.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(Duration::from_millis(100));
        let rms = f32::from_bits(rms_bits.load(Ordering::Relaxed));
        let peak = f32::from_bits(peak_bits.load(Ordering::Relaxed));
        let count = sample_count.load(Ordering::Relaxed);

        let bars = ((rms * 200.0).clamp(0.0, 50.0)) as usize;
        let s: String = "#".repeat(bars);
        let dots: String = "-".repeat(50 - bars);
        print!("\r[{s}{dots}] rms={rms:.4} peak={peak:.3} ");
        let _ = std::io::stdout().flush();

        if count == last_count {
            stale_ticks += 1;
        } else {
            stale_ticks = 0;
        }
        last_count = count;
    }
    println!();

    if sample_count.load(Ordering::Relaxed) == 0 {
        println!("\n⚠️  Aucun échantillon reçu — le périphérique ne pousse pas d'audio.");
        println!(
            "    Causes typiques: micro muet (PulseAudio/PipeWire), permissions, mauvais device par défaut."
        );
    } else if stale_ticks > 20 {
        println!(
            "\n⚠️  Le flux s'est figé en cours de route (callback cpal qui ne pousse plus de buffers)."
        );
    } else {
        let peak = f32::from_bits(peak_bits.load(Ordering::Relaxed));
        if peak < 0.01 {
            println!(
                "\n⚠️  Pic max très bas ({peak:.4}) — le micro reçoit du signal numérique mais quasiment muet."
            );
            println!("    Vérifiez le gain / boost dans pavucontrol ou alsamixer.");
        } else {
            println!("\n✅ Audio capté correctement (pic={peak:.3}).");
        }
    }

    Ok(())
}
