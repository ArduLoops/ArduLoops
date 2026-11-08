// FilterReview: Gyro noise analysis, harmonic notch parameter optimization and virtual filter simulation.

/// Computes the complex frequency response H(f) of a digital biquad notch filter,
/// exactly matching ArduPilot WebTools FilterReview.js and ArduPilot HarmonicNotchFilter.
#[cfg(test)]
pub fn notch_response(f0: f64, bw: f64, fs: f64, f: f64) -> (f64, f64) {
    notch_response_att(f0, bw, 40.0, fs, f)
}

/// The same notch with its depth set by `att_db` (INS_HNTCH_ATT): A = 10^(-att/40).
pub fn notch_response_att(f0: f64, bw: f64, att_db: f64, fs: f64, f: f64) -> (f64, f64) {
    if fs <= 0.0 || f0 <= 0.0 || bw <= 0.0 || f0 <= 0.5 * bw || f0 >= 0.5 * fs {
        return (1.0, 0.0);
    }
    let pi = std::f64::consts::PI;
    let octaves = (f0 / (f0 - bw * 0.5)).log2() * 2.0;
    let pow2 = 2.0f64.powf(octaves);
    let q = (pow2.sqrt() / (pow2 - 1.0)).clamp(0.1, 100.0);

    // Attenuation: -40 dB -> A = 10^(-40/40) = 0.1
    let a = 10f64.powf(-att_db / 40.0);
    let a_sq = a * a;

    let omega0 = 2.0 * pi * f0 / fs;
    let alpha = omega0.sin() / (2.0 * q);

    let b0 = 1.0 + alpha * a_sq;
    let b1 = -2.0 * omega0.cos();
    let b2 = 1.0 - alpha * a_sq;
    let a0 = 1.0 + alpha;
    let a1 = b1;
    let a2 = 1.0 - alpha;

    let w = 2.0 * pi * f / fs;
    let cos_w = w.cos();
    let sin_w = w.sin();
    let cos_2w = (2.0 * w).cos();
    let sin_2w = (2.0 * w).sin();

    let num_re = b0 + b1 * cos_w + b2 * cos_2w;
    let num_im = -b1 * sin_w - b2 * sin_2w;

    let den_re = a0 + a1 * cos_w + a2 * cos_2w;
    let den_im = -a1 * sin_w - a2 * sin_2w;

    let den_mag2 = den_re * den_re + den_im * den_im;
    if den_mag2 < 1e-12 {
        return (0.0, 0.0);
    }

    let h_re = (num_re * den_re + num_im * den_im) / den_mag2;
    let h_im = (num_im * den_re - num_re * den_im) / den_mag2;
    let gain = (h_re * h_re + h_im * h_im).sqrt();
    let phase = h_im.atan2(h_re);
    (gain, phase)
}

/// Computes the frequency response of a 2nd order Butterworth Low-Pass Filter,
/// matching ArduPilot DigitalBiquadFilter in WebTools FilterReview.js.
pub fn lpf_response(cutoff_hz: f64, fs: f64, f: f64) -> (f64, f64) {
    if fs <= 0.0 || cutoff_hz <= 0.0 || cutoff_hz >= fs * 0.5 {
        return (1.0, 0.0);
    }
    let pi = std::f64::consts::PI;
    let fr = fs / cutoff_hz;
    let ohm = (pi / fr).tan();
    let c = 1.0 + 2.0 * (pi / 4.0).cos() * ohm + ohm * ohm;

    let b0 = (ohm * ohm) / c;
    let b1 = 2.0 * b0;
    let b2 = b0;
    let a1 = 2.0 * (ohm * ohm - 1.0) / c;
    let a2 = (1.0 - 2.0 * (pi / 4.0).cos() * ohm + ohm * ohm) / c;

    let w = 2.0 * pi * f / fs;
    let cos_w = w.cos();
    let sin_w = w.sin();
    let cos_2w = (2.0 * w).cos();
    let sin_2w = (2.0 * w).sin();

    let num_re = b0 + b1 * cos_w + b2 * cos_2w;
    let num_im = -b1 * sin_w - b2 * sin_2w;

    let den_re = 1.0 + a1 * cos_w + a2 * cos_2w;
    let den_im = -a1 * sin_w - a2 * sin_2w;

    let den_mag2 = den_re * den_re + den_im * den_im;
    if den_mag2 < 1e-12 {
        return (0.0, 0.0);
    }

    let h_re = (num_re * den_re + num_im * den_im) / den_mag2;
    let h_im = (num_im * den_re - num_re * den_im) / den_mag2;
    let gain = (h_re * h_re + h_im * h_im).sqrt();
    let phase = h_im.atan2(h_re);
    (gain, phase)
}

/// Simulated harmonic notch filter performance.
#[derive(Clone, Debug)]
pub struct FilterSimResult {
    pub noise_attenuation_db: f64,
    pub phase_delay_at_30hz_ms: f64,
}

/// Main entry point for log_filter_review calculation from log bytes.
pub fn filter_review_bytes(
    bytes: &[u8],
    axis: Option<&str>,
    start_us: Option<u64>,
    end_us: Option<u64>,
) -> Value {
    let formats = parse_formats(bytes);
    if formats.is_empty() {
        return json!({ "ok": false, "error": "No message format headers found in log" });
    }

    let mut parms: BTreeMap<String, f64> = BTreeMap::new();
    let mut ftn_freqs = Vec::new();
    let mut ftn_snrs = Vec::new();
    let mut throttles = Vec::new();
    let mut rpms = Vec::new();
    let mut motor_rpm: BTreeMap<u64, Vec<(u64, f64)>> = BTreeMap::new();
    let mut tracked: Vec<(u64, f64)> = Vec::new();
    let mut rate_gyr = Vec::new();
    let mut first_tho_us = 0u64;
    let mut last_tho_us = 0u64;

    let axis_filter = axis.unwrap_or("all").to_ascii_lowercase();

    let mut incomplete = false;
    walk(bytes, &formats, |fmt, _, values| {
        let Some(us) = time_us(fmt, values) else { return };
        if start_us.is_some_and(|s| us < s) || end_us.is_some_and(|e| us > e) {
            return;
        }

        if fmt.name == "PARM" {
            if let (Some(name), Some(val)) = (field_str(fmt, values, "Name"), field_f64(fmt, values, "Value")) {
                if name.starts_with("INS_HNTCH_") || name.starts_with("INS_HNTC2_") || name.starts_with("INS_GYRO_FILTER") {
                    parms.insert(name, val);
                }
            }
        } else if fmt.name == "FTN1" || fmt.name == "FTN2" {
            if let Some(freq) = field_f64(fmt, values, "PkAvg")
                .or_else(|| field_f64(fmt, values, "Freq"))
                .or_else(|| field_f64(fmt, values, "NF"))
            {
                if freq > 30.0 && freq < 600.0 {
                    let snr = field_f64(fmt, values, "SnrAvg").unwrap_or(10.0);
                    ftn_freqs.push((us, freq));
                    ftn_snrs.push(snr);
                }
            }
        } else if fmt.name == "CTUN" {
            if let Some(tho) = field_f64(fmt, values, "ThO") {
                if tho > 0.05 && tho < 0.95 {
                    if first_tho_us == 0 {
                        first_tho_us = us;
                    }
                    last_tho_us = us;
                    throttles.push((us, tho));
                }
            }
        } else if fmt.name == "ESC" {
            if let Some(rpm) = field_f64(fmt, values, "RPM") {
                if rpm > 500.0 {
                    rpms.push((us, rpm));
                    let motor = field_u64(fmt, values, "Instance").unwrap_or(0);
                    motor_rpm.entry(motor).or_default().push((us, rpm));
                }
            }
        } else if fmt.name == "FTNS" {
            if let Some(nf) = field_f64(fmt, values, "NF") {
                if nf > 0.0 {
                    tracked.push((us, nf));
                }
            }
        } else if fmt.name == "RATE" {
            let col = match axis_filter.as_str() {
                "pitch" => "P",
                "yaw" => "Y",
                _ => "R",
            };
            if let Some(val) = field_f64(fmt, values, col) {
                rate_gyr.push((us, val));
            }
        }
    }, &mut incomplete);

    // If batch gyro sampling is present, run gyro_batch_fft to get exact spectral envelope
    let scan = scan_gyro(bytes);
    let views = classify_captures(&scan.groups);

    let (sample_hz, spectrum, detected_peak_hz, _detected_snr) = if !views.is_empty() {
        let primary = &views[0];
        let rate = primary["sample_hz"].as_f64().unwrap_or(1000.0);
        let peak = primary["fundamental_hz"].as_f64().unwrap_or(0.0);
        let amp = primary["fundamental_amplitude"].as_f64().unwrap_or(0.0);
        let mut spec = Vec::new();
        if let Some(env) = primary["envelope"].as_array() {
            let res = primary["resolution_hz"].as_f64().unwrap_or(1.0);
            for (idx, val) in env.iter().enumerate() {
                if let Some(v) = val.as_f64() {
                    spec.push((idx as f64 * res, v));
                }
            }
        }
        (rate, spec, peak, amp)
    } else if !ftn_freqs.is_empty() {
        // Construct spectrum from in-flight FFT tracking (FTN1)
        let mean_freq = ftn_freqs.iter().map(|f| f.1).sum::<f64>() / ftn_freqs.len() as f64;
        let mean_snr = if ftn_snrs.is_empty() { 15.0 } else { ftn_snrs.iter().sum::<f64>() / ftn_snrs.len() as f64 };
        let mut spec = Vec::new();
        for f in (20..500).step_by(5) {
            let f_f64 = f as f64;
            let dist = (f_f64 - mean_freq).abs();
            let amp = (-dist * dist / 200.0).exp();
            spec.push((f_f64, amp));
        }
        (1000.0, spec, mean_freq, mean_snr)
    } else if rate_gyr.len() > 64 {
        // Fast FFT from RATE message gyro field
        let n = next_pow2(rate_gyr.len().min(4096));
        let mut dts: Vec<f64> = rate_gyr.windows(2).map(|w| (w[1].0.saturating_sub(w[0].0)) as f64 / 1_000_000.0).filter(|d| *d > 0.0).collect();
        dts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let dt = if dts.is_empty() { 0.0025 } else { dts[dts.len() / 2] };
        let s_rate = 1.0 / dt;
        let mut re = vec![0.0f64; n];
        let mut im = vec![0.0f64; n];
        let mean = rate_gyr[..n].iter().map(|s| s.1).sum::<f64>() / n as f64;
        for (i, item) in rate_gyr[..n].iter().enumerate() {
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
            re[i] = (item.1 - mean) * w;
        }
        fft(&mut re, &mut im, false);
        let mut spec = Vec::new();
        let mut best_hz = 0.0;
        let mut best_amp = 0.0;
        let res = s_rate / n as f64;
        for i in 1..(n / 2) {
            let f = i as f64 * res;
            let a = (re[i] * re[i] + im[i] * im[i]).sqrt() / n as f64;
            spec.push((f, a));
            if f >= 35.0 && f <= 450.0 && a > best_amp {
                best_amp = a;
                best_hz = f;
            }
        }
        (s_rate, spec, best_hz, best_amp)
    } else {
        return json!({
            "ok": false,
            "error": "No gyro vibration data found in log. Enable batch logging (INS_LOG_BAT_OPT=1 or 2) or ensure RATE/FTN1 messages are recorded."
        });
    };

    // Calculate hover throttle if available
    let hover_tho = if throttles.is_empty() {
        0.30
    } else {
        let mut sorted_tho: Vec<f64> = throttles.iter().map(|t| t.1).collect();
        sorted_tho.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        sorted_tho[sorted_tho.len() / 2]
    };

    let (fundamental_amp, resolution) = spectrum_peak(&spectrum, detected_peak_hz);
    let post_view = views.iter().find(|view| view["capture"].as_str() == Some("post-filter")).cloned();
    let median = |values: &mut Vec<f64>| -> Option<f64> {
        if values.is_empty() {
            return None;
        }
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        Some(values[values.len() / 2])
    };
    let tracked_hz = median(&mut tracked.iter().map(|(_, hz)| *hz).collect());

    let get = |name: &str, default: f64| parms.get(name).copied().unwrap_or(default);
    let cur = NotchPlan {
        enable: get("INS_HNTCH_ENABLE", 0.0) >= 0.5,
        mode: get("INS_HNTCH_MODE", 0.0).round() as u32,
        freq: get("INS_HNTCH_FREQ", 0.0),
        bw: get("INS_HNTCH_BW", 0.0),
        att: get("INS_HNTCH_ATT", 40.0),
        hmncs: get("INS_HNTCH_HMNCS", 1.0).round().max(0.0) as u32,
        opts: get("INS_HNTCH_OPTS", 0.0).round().max(0.0) as u32,
        lpf: get("INS_GYRO_FILTER", 20.0),
    };

    // Where the notch sits in flight: a dynamic notch follows the motors (FTNS), a static one sits at FREQ.
    let base_hz = if cur.enable && cur.mode != 0 { tracked_hz.or(Some(detected_peak_hz)).unwrap_or(cur.freq) } else { cur.freq };
    let motor_hz: Vec<f64> = motor_rpm.values().filter_map(|rows| median(&mut rows.iter().map(|(_, rpm)| *rpm).collect())).map(|rpm| rpm / 60.0).collect();
    // The batches the simulation runs on: each pre-filter batch with the notch where it was then.
    let batches = BatchSet::new(&scan, views.first(), &spectrum, sample_hz, &tracked, &motor_rpm);
    let spread_hz = if motor_hz.len() >= 2 {
        motor_hz.iter().copied().fold(f64::MIN, f64::max) - motor_hz.iter().copied().fold(f64::MAX, f64::min)
    } else {
        0.0
    };

    // Each harmonic of the motor line: its level against the fundamental and whether a notch covers it.
    let nyquist = sample_hz * 0.5;
    let mut harmonics = Vec::new();
    for n in 1..=6u32 {
        let hz = detected_peak_hz * n as f64;
        if detected_peak_hz <= 0.0 || hz >= nyquist * 0.9 {
            break;
        }
        // A narrow window: the line itself, not the broadband noise around it.
        let (amp, _) = spectrum_peak_near(&spectrum, hz, (hz * 0.04).max(resolution * 2.0));
        let level = if amp <= 0.0 || fundamental_amp <= 0.0 { -120.0 } else { 20.0 * (amp / fundamental_amp).log10() };
        let covered = cur.enable && cur.hmncs & (1 << (n - 1)) != 0;
        harmonics.push(json!({ "n": n, "hz": (hz * 10.0).round() / 10.0, "level_db": (level * 10.0).round() / 10.0, "notched": covered }));
    }

    // Proposal: of the settings that leave no more noise after the filters than now, the one with the
    // least delay at 30 Hz (fewest notches on a tie). It may add notches on loud harmonics, Multi-Source
    // (ESC RPM only), and a higher gyro low-pass.
    let mut base = cur.clone();
    base.enable = true;
    if !cur.enable {
        base.mode = if !rpms.is_empty() { 3 } else { 1 };
        base.freq = detected_peak_hz.round().max(30.0);
        base.bw = (base.freq * 0.5).round().max(20.0);
        base.att = 40.0;
        base.hmncs = 1;
    } else if cur.mode == 0 && (cur.freq - detected_peak_hz).abs() > cur.bw.max(20.0) * 0.5 && detected_peak_hz > 30.0 {
        base.freq = detected_peak_hz.round();
        base.bw = (base.freq * 0.5).round().max(20.0);
    }
    let rec_base = if base.mode != 0 { tracked_hz.unwrap_or(detected_peak_hz) } else { base.freq };
    let cur_centres = notch_centres(&cur, base_hz, &motor_hz);
    let cur_left = batches.after_notches(&cur, base_hz, &motor_hz);
    let cur_noise = batches.residual(&cur_left, cur.lpf);
    let cur_delay = plan_result(&cur, &cur_centres, sample_hz, &spectrum).phase_delay_at_30hz_ms;
    let extra: Vec<u32> = harmonics.iter()
        .filter_map(|row| Some((row["n"].as_u64()? as u32, row["level_db"].as_f64()?)))
        .filter(|(n, level)| *level >= HARMONIC_LOUD_DB && base.hmncs & (1 << (n - 1)) == 0)
        .map(|(n, _)| 1 << (n - 1))
        .collect();
    let multi_options: &[bool] = if base.mode == 3 && motor_hz.len() >= 2 { &[false, true] } else { &[false] };
    let max_lpf = (sample_hz * 0.25).min(LPF_CEILING_HZ);
    let mut best: Option<(NotchPlan, f64)> = None;
    for subset in 0..(1u32 << extra.len().min(4)) {
        let mut mask = base.hmncs;
        for (i, bit) in extra.iter().take(4).enumerate() {
            if subset & (1 << i) != 0 {
                mask |= bit;
            }
        }
        for &multi in multi_options {
            let mut plan = base.clone();
            plan.hmncs = mask;
            plan.opts = if multi { base.opts | MULTI_SOURCE } else { base.opts & !MULTI_SOURCE };
            let centres = notch_centres(&plan, rec_base, &motor_hz);
            let left = batches.after_notches(&plan, rec_base, &motor_hz);
            let mut lpf = cur.lpf;
            let mut step = (cur.lpf / 5.0).floor() * 5.0 + 5.0;
            while step <= max_lpf {
                if batches.residual(&left, step) <= cur_noise {
                    lpf = step;
                }
                step += 5.0;
            }
            plan.lpf = lpf;
            if batches.residual(&left, plan.lpf) > cur_noise * 1.0001 {
                continue;
            }
            let delay = plan_result(&plan, &centres, sample_hz, &spectrum).phase_delay_at_30hz_ms;
            let better = match &best {
                None => true,
                Some((kept, kept_delay)) => delay < kept_delay - 0.05
                    || ((delay - kept_delay).abs() <= 0.05 && centres.len() < notch_centres(kept, rec_base, &motor_hz).len()),
            };
            if better {
                best = Some((plan, delay));
            }
        }
    }
    // Keep the settings on the vehicle unless a change takes off real delay.
    let rec = match best {
        Some((plan, delay)) if !cur.enable || delay < cur_delay - 0.2 => plan,
        _ => base,
    };
    let rec_centres = notch_centres(&rec, rec_base, &motor_hz);
    let rec_left = batches.after_notches(&rec, rec_base, &motor_hz);
    let cur_sim = FilterSimResult {
        noise_attenuation_db: batches.attenuation_db(&cur_left, cur.lpf),
        phase_delay_at_30hz_ms: cur_delay,
    };
    let rec_sim = FilterSimResult {
        noise_attenuation_db: batches.attenuation_db(&rec_left, rec.lpf),
        phase_delay_at_30hz_ms: plan_result(&rec, &rec_centres, sample_hz, &spectrum).phase_delay_at_30hz_ms,
    };

    let mut proposed = serde_json::Map::new();
    let mut put = |name: &str, now: f64, next: f64| {
        if (now - next).abs() > 1e-6 {
            proposed.insert(name.to_string(), json!(next));
        }
    };
    put("INS_HNTCH_ENABLE", if cur.enable { 1.0 } else { 0.0 }, 1.0);
    put("INS_HNTCH_MODE", cur.mode as f64, rec.mode as f64);
    put("INS_HNTCH_FREQ", cur.freq, rec.freq);
    put("INS_HNTCH_BW", cur.bw, rec.bw);
    put("INS_HNTCH_HMNCS", cur.hmncs as f64, rec.hmncs as f64);
    put("INS_HNTCH_OPTS", cur.opts as f64, rec.opts as f64);
    put("INS_GYRO_FILTER", cur.lpf, rec.lpf);
    let needs_update = !proposed.is_empty();

    let measured = post_view.map(|view| json!({
        "attenuation_db": view["attenuation_db"],
        "harmonic_attenuation_db": view["harmonic_attenuation_db"],
    }));

    let chart_end = if last_tho_us > first_tho_us { (first_tho_us + 10_000_000).min(last_tho_us) } else { first_tho_us + 10_000_000 };
    let suggested_chart = json!({
        "title": "Throttle Profile for Notch Tracking",
        "start_us": first_tho_us,
        "end_us": chart_end,
        "axes": [{
            "label": "Throttle (0..1)",
            "lines": [
                { "message": "CTUN", "field": "ThO", "name": "Throttle Output" }
            ]
        }]
    });

    json!({
        "ok": true,
        "hover_interval": {
            "start_us": first_tho_us,
            "end_us": last_tho_us
        },
        "suggested_chart": suggested_chart,
        "sample_rate_hz": (sample_hz * 10.0).round() / 10.0,
        "primary_peak_hz": (detected_peak_hz * 10.0).round() / 10.0,
        "tracked_notch_hz": tracked_hz.map(|hz| (hz * 10.0).round() / 10.0),
        "motor_hz": motor_hz.iter().map(|hz| (hz * 10.0).round() / 10.0).collect::<Vec<_>>(),
        "motor_spread_hz": (spread_hz * 10.0).round() / 10.0,
        "estimated_hover_throttle": (hover_tho * 100.0).round() / 100.0,
        "harmonics": harmonics,
        "current_params": {
            "INS_HNTCH_ENABLE": cur.enable as u32,
            "INS_HNTCH_MODE": cur.mode,
            "INS_HNTCH_FREQ": cur.freq,
            "INS_HNTCH_BW": cur.bw,
            "INS_HNTCH_ATT": cur.att,
            "INS_HNTCH_HMNCS": cur.hmncs,
            "INS_HNTCH_OPTS": cur.opts,
            "INS_GYRO_FILTER": cur.lpf
        },
        "proposed_params": Value::Object(proposed),
        "simulation": {
            "current_attenuation_db": cur_sim.noise_attenuation_db,
            "current_phase_delay_ms": cur_sim.phase_delay_at_30hz_ms,
            "proposed_attenuation_db": rec_sim.noise_attenuation_db,
            "proposed_phase_delay_ms": rec_sim.phase_delay_at_30hz_ms
        },
        "measured_post_filter": measured,
        "needs_update": needs_update,
        "note": concat!(
            "harmonics: each multiple of the motor line in the pre-filter spectrum, level_db against the fundamental, and notched when INS_HNTCH_HMNCS covers it; a harmonic that is not notched reaches the controller through the gyro low-pass only. ",
            "motor_hz is each motor's median ESC RPM / 60; motor_spread_hz is the gap between the fastest and slowest. One notch at their average leaves the edges of the peak; Multi-Source (INS_HNTCH_OPTS bit 1) puts one notch on each motor and needs the ESC RPM source. ",
            "proposed_params lists only values that change. Of the settings that leave no more simulated noise after all filters than the current ones, it is the one with the least delay at 30 Hz: it may notch harmonics louder than -30 dB, turn on Multi-Source, and raise INS_GYRO_FILTER. Empty means the current settings already are that. ",
            "attenuation_db is the noise above 20 Hz removed by all filters together, simulated batch by batch with the notch where it tracked at that moment, as WebTools FilterReview does; phase_delay_ms is their delay at 30 Hz, the lag the rate loop sees. ",
            "measured_post_filter is the post-filter batch capture, measured not simulated: attenuation of the motor line and of each harmonic between the raw and the filtered gyro. ",
            "A simulation uses one log; the next flight's spectrum is what confirms a change."
        ),
    })
}

/// The sound a harmonic must have, against the fundamental, to be worth a notch.
const HARMONIC_LOUD_DB: f64 = -30.0;
/// INS_HNTCH_OPTS bit 1: one notch per ESC RPM source.
const MULTI_SOURCE: u32 = 1 << 1;
/// The gyro low-pass is not proposed above this.
const LPF_CEILING_HZ: f64 = 120.0;

#[derive(Clone)]
struct NotchPlan {
    enable: bool,
    mode: u32,
    freq: f64,
    bw: f64,
    att: f64,
    hmncs: u32,
    opts: u32,
    lpf: f64,
}

/// The notch centres and widths in flight: each enabled harmonic of the base (or of each motor with
/// Multi-Source). The quality factor is fixed across harmonics, so a width scales with its centre.
fn notch_centres(plan: &NotchPlan, base_hz: f64, motor_hz: &[f64]) -> Vec<(f64, f64)> {
    if !plan.enable || plan.freq <= 0.0 || plan.bw <= 0.0 {
        return Vec::new();
    }
    let sources: Vec<f64> = if plan.opts & MULTI_SOURCE != 0 && plan.mode == 3 && !motor_hz.is_empty() {
        motor_hz.to_vec()
    } else {
        vec![base_hz]
    };
    let mut out = Vec::new();
    for source in sources {
        for bit in 0..8 {
            if plan.hmncs & (1 << bit) == 0 {
                continue;
            }
            let centre = source * (bit + 1) as f64;
            out.push((centre, centre * plan.bw / plan.freq));
        }
    }
    out
}

fn plan_gain(plan: &NotchPlan, centres: &[(f64, f64)], lpf: f64, rate: f64, f: f64) -> (f64, f64) {
    let mut gain = 1.0;
    let mut phase = 0.0;
    for (centre, bw) in centres {
        let (g, p) = notch_response_att(*centre, *bw, plan.att, rate, f);
        gain *= g;
        phase += p;
    }
    if lpf > 0.0 {
        let (g, p) = lpf_response(lpf, rate, f);
        gain *= g;
        phase += p;
    }
    (gain, phase)
}

/// The noise power above 20 Hz left after the filters.
fn residual_power(plan: &NotchPlan, centres: &[(f64, f64)], lpf: f64, rate: f64, spectrum: &[(f64, f64)]) -> f64 {
    spectrum.iter().filter(|(f, _)| *f >= 20.0).map(|(f, amp)| {
        let (gain, _) = plan_gain(plan, centres, lpf, rate, *f);
        amp * amp * gain * gain
    }).sum()
}

fn plan_result(plan: &NotchPlan, centres: &[(f64, f64)], rate: f64, spectrum: &[(f64, f64)]) -> FilterSimResult {
    let input: f64 = spectrum.iter().filter(|(f, _)| *f >= 20.0).map(|(_, amp)| amp * amp).sum();
    let output = residual_power(plan, centres, plan.lpf, rate, spectrum);
    let attenuation = if input > 1e-12 && output > 1e-12 { 10.0 * (input / output).log10() } else { 0.0 };
    let (_, phase) = plan_gain(plan, centres, plan.lpf, rate, 30.0);
    let delay_ms = (-phase / (2.0 * std::f64::consts::PI * 30.0) * 1000.0).max(0.0);
    FilterSimResult {
        noise_attenuation_db: (attenuation * 10.0).round() / 10.0,
        phase_delay_at_30hz_ms: (delay_ms * 10.0).round() / 10.0,
    }
}

/// The pre-filter batches with the notch frequency of their moment. A log without a gyro batch
/// falls back to one batch: the averaged spectrum at the median notch frequency.
struct BatchSet {
    rate: f64,
    hz: Vec<f64>,
    /// Per batch: power per bin, the tracked notch base (Hz), and each motor's frequency (Hz).
    rows: Vec<(Vec<f64>, Option<f64>, Vec<f64>)>,
}

impl BatchSet {
    fn new(
        scan: &GyroScan,
        primary: Option<&Value>,
        spectrum: &[(f64, f64)],
        rate: f64,
        tracked: &[(u64, f64)],
        motor_rpm: &BTreeMap<u64, Vec<(u64, f64)>>,
    ) -> Self {
        let mut tracked = tracked.to_vec();
        tracked.sort_by_key(|row| row.0);
        let motors: Vec<Vec<(u64, f64)>> = motor_rpm.values().map(|rows| {
            let mut rows = rows.clone();
            rows.sort_by_key(|row| row.0);
            rows
        }).collect();
        let group = primary.and_then(|view| view["instance"].as_u64()).and_then(|id| scan.groups.get(&id));
        if let Some(group) = group.filter(|group| !group.power.is_empty()) {
            let bins = group.power[0].1.len();
            let hz = (0..bins).map(|i| group.rate * i as f64 / group.fft_n as f64).collect();
            let rows = group.power.iter().filter(|(_, p)| p.len() == bins).map(|(t, p)| {
                let at = nearest(&tracked, *t, 500_000);
                let each = motors.iter().filter_map(|rows| nearest(rows, *t, 500_000).map(|rpm| rpm / 60.0)).collect();
                (p.clone(), at, each)
            }).collect();
            return BatchSet { rate: group.rate, hz, rows };
        }
        BatchSet {
            rate,
            hz: spectrum.iter().map(|(f, _)| *f).collect(),
            rows: vec![(spectrum.iter().map(|(_, a)| a * a).collect(), None, Vec::new())],
        }
    }

    /// Power per bin left after the notches, summed over the batches. Each batch has the notch
    /// where it was then: the tracked frequency, or each motor's with Multi-Source.
    fn after_notches(&self, plan: &NotchPlan, fallback_hz: f64, fallback_motors: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; self.hz.len()];
        for (power, tracked, motors) in &self.rows {
            let base = if plan.mode != 0 { tracked.unwrap_or(fallback_hz) } else { plan.freq };
            let motors = if motors.is_empty() { fallback_motors } else { motors.as_slice() };
            let centres = notch_centres(plan, base, motors);
            for (i, f) in self.hz.iter().enumerate() {
                if *f < 20.0 {
                    continue;
                }
                let mut gain = 1.0;
                for (centre, bw) in &centres {
                    gain *= notch_response_att(*centre, *bw, plan.att, self.rate, *f).0;
                }
                out[i] += power[i] * gain * gain;
            }
        }
        out
    }

    /// The noise power above 20 Hz after the notches and the gyro low-pass.
    fn residual(&self, after_notches: &[f64], lpf: f64) -> f64 {
        self.hz.iter().zip(after_notches).filter(|(f, _)| **f >= 20.0).map(|(f, p)| {
            let g = if lpf > 0.0 { lpf_response(lpf, self.rate, *f).0 } else { 1.0 };
            p * g * g
        }).sum()
    }

    fn attenuation_db(&self, after_notches: &[f64], lpf: f64) -> f64 {
        let input: f64 = self.rows.iter().map(|(p, _, _)| {
            self.hz.iter().zip(p).filter(|(f, _)| **f >= 20.0).map(|(_, v)| *v).sum::<f64>()
        }).sum();
        let output = self.residual(after_notches, lpf);
        let db = if input > 1e-18 && output > 1e-18 { 10.0 * (input / output).log10() } else { 0.0 };
        (db * 10.0).round() / 10.0
    }
}

/// The amplitude of the spectrum at `hz` and its bin width.
fn spectrum_peak(spectrum: &[(f64, f64)], hz: f64) -> (f64, f64) {
    let resolution = spectrum.windows(2).next().map(|w| w[1].0 - w[0].0).unwrap_or(1.0).max(1e-6);
    (spectrum_peak_near(spectrum, hz, resolution * 2.0).0, resolution)
}

/// The largest amplitude within `half` Hz of `hz`.
fn spectrum_peak_near(spectrum: &[(f64, f64)], hz: f64, half: f64) -> (f64, f64) {
    spectrum.iter().filter(|(f, _)| (*f - hz).abs() <= half).fold((0.0, hz), |best, (f, amp)| if *amp > best.0 { (*amp, *f) } else { best })
}

/// Public API called by agent or catalog.
pub fn filter_review(
    id: &str,
    axis: Option<&str>,
    start_us: Option<u64>,
    end_us: Option<u64>,
) -> Value {
    let bytes = match read_id(id) {
        Ok(b) => b,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    filter_review_bytes(&bytes, axis, start_us, end_us)
}
