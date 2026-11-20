// PIDReview: the measured closed-loop response of a rate loop.
// One transfer estimate, actual over target, gives the step response, the Bode plot, and the margins.

/// Axis name, RATE desired and actual columns, the ATC_RAT_ stem, the PID message, the ATT desired
/// angle, the quadplane VTOL PID message, and the fixed-wing gain prefix.
const PID_AXES: [(&str, &str, &str, &str, &str, &str, &str, &str); 3] = [
    ("roll", "RDes", "R", "RLL", "PIDR", "DesRoll", "PIQR", "RLL_RATE_"),
    ("pitch", "PDes", "P", "PIT", "PIDP", "DesPitch", "PIQP", "PTCH_RATE_"),
    ("yaw", "YDes", "Y", "YAW", "PIDY", "DesYaw", "PIQY", "YAW_RATE_"),
];
/// The rate gains of every vehicle this reads: copter, quadplane VTOL, and fixed wing.
const PID_GAIN_PREFIXES: [&str; 5] = ["ATC_RAT_", "Q_A_RAT_", "RLL_RATE_", "PTCH_RATE_", "YAW_RATE_"];

/// Length of the step response that is estimated and drawn for a multicopter rate loop.
const PID_STEP_S: f64 = 0.5;
/// The same for a fixed-wing rate loop, which is slower.
const PID_STEP_WING_S: f64 = 1.0;
/// Below this sample rate a step response has too few points to draw.
const PID_STEP_MIN_HZ: f64 = 50.0;
/// The Bode plot stops here or at 0.45 of the sample rate.
const PID_BODE_MAX_HZ: f64 = 150.0;
/// Relative standard error up to which a point of the estimate is a measurement.
const PID_ERR_OK: f64 = 0.25;
/// The tighter limit for the points a margin is read from.
const PID_ERR_TIGHT: f64 = 0.15;

struct PidScan {
    parms: BTreeMap<String, f64>,
    /// RATE per axis: (us, desired, actual) in deg/s.
    rate: [Vec<(u64, f64, f64)>; 3],
    /// PIDR, PIDP, PIDY: (us, Tar, Act).
    pid: [Vec<(u64, f64, f64)>; 3],
    /// PIQR, PIQP, PIQY, the quadplane VTOL loops: (us, Tar, Act).
    piq: [Vec<(u64, f64, f64)>; 3],
    /// ATT desired angle per axis: (us, degrees).
    att: [Vec<(u64, f64)>; 3],
    /// The firmware line the log starts with, such as "ArduCopter V4.6.2 (1234abcd)".
    firmware: String,
    /// ArduCopter System ID runs: (start_us, end_us, SID_AXIS), from "SystemID Starting: axis=N" to
    /// "SystemID Finished" or the change out of System ID mode, whichever comes first.
    system_id: Vec<(u64, u64, u64)>,
}

fn pid_axis_index(axis: Option<&str>) -> usize {
    let name = axis.unwrap_or("roll").to_ascii_lowercase();
    PID_AXES.iter().position(|row| row.0 == name).unwrap_or(0)
}

fn pid_scan(bytes: &[u8], start_us: Option<u64>, end_us: Option<u64>) -> Option<PidScan> {
    let formats = parse_formats(bytes);
    if formats.is_empty() {
        return None;
    }
    let mut scan = PidScan {
        parms: BTreeMap::new(),
        rate: [Vec::new(), Vec::new(), Vec::new()],
        pid: [Vec::new(), Vec::new(), Vec::new()],
        piq: [Vec::new(), Vec::new(), Vec::new()],
        att: [Vec::new(), Vec::new(), Vec::new()],
        firmware: String::new(),
        system_id: Vec::new(),
    };
    let mut incomplete = false;
    walk_only(bytes, &formats, &["MSG", "MODE", "PARM", "RATE", "ATT", "PIDR", "PIDP", "PIDY", "PIQR", "PIQP", "PIQY"], |fmt, _, values| {
        if fmt.name == "MSG" {
            let text = field_str(fmt, values, "Message").unwrap_or_default();
            if scan.firmware.is_empty() && text.starts_with("Ardu") {
                scan.firmware = text.clone();
            }
            let us = time_us(fmt, values).unwrap_or(0);
            if let Some(axis) = text.strip_prefix("SystemID Starting: axis=").and_then(|n| n.trim().parse::<u64>().ok()) {
                scan.system_id.push((us, 0, axis));
            } else if text.starts_with("SystemID Finished") {
                if let Some(run) = scan.system_id.last_mut().filter(|run| run.1 == 0) {
                    run.1 = us;
                }
            }
            return;
        }
        // Leaving System ID mode (25) ends a run whose "SystemID Finished" was not logged.
        if fmt.name == "MODE" {
            let mode = field_u64(fmt, values, "ModeNum").or_else(|| field_u64(fmt, values, "Mode"));
            if mode.is_some_and(|m| m != 25) {
                let us = time_us(fmt, values).unwrap_or(0);
                if let Some(run) = scan.system_id.last_mut().filter(|run| run.1 == 0) {
                    run.1 = us;
                }
            }
            return;
        }
        // Parameters are written at the start of the log, before any interval.
        if fmt.name == "PARM" {
            if let (Some(name), Some(val)) = (field_str(fmt, values, "Name"), field_f64(fmt, values, "Value")) {
                if PID_GAIN_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) || PID_LOG_PARAMS.contains(&name.as_str()) {
                    scan.parms.insert(name, val);
                }
            }
            return;
        }
        let pid_axis = PID_AXES.iter().position(|row| row.4 == fmt.name || row.6 == fmt.name);
        if pid_axis.is_none() && fmt.name != "RATE" && fmt.name != "ATT" {
            return;
        }
        let Some(us) = time_us(fmt, values) else { return };
        if start_us.is_some_and(|s| us < s) || end_us.is_some_and(|e| us > e) {
            return;
        }
        if let Some(axis) = pid_axis {
            if let (Some(tar), Some(act)) = (field_f64(fmt, values, "Tar"), field_f64(fmt, values, "Act")) {
                if tar.is_finite() && act.is_finite() {
                    let series = if fmt.name == PID_AXES[axis].6 { &mut scan.piq } else { &mut scan.pid };
                    series[axis].push((us, tar, act));
                }
            }
            return;
        }
        for (axis, row) in PID_AXES.iter().enumerate() {
            if fmt.name == "ATT" {
                if let Some(des) = field_f64(fmt, values, row.5).filter(|v| v.is_finite()) {
                    scan.att[axis].push((us, des));
                }
            } else if let (Some(des), Some(act)) = (field_f64(fmt, values, row.1), field_f64(fmt, values, row.2)) {
                if des.is_finite() && act.is_finite() {
                    scan.rate[axis].push((us, des, act));
                }
            }
        }
    }, &mut incomplete);
    Some(scan)
}

fn pid_median_dt(samples: &[(u64, f64, f64)]) -> Option<f64> {
    let mut dts: Vec<f64> = samples.windows(2)
        .map(|w| w[1].0.saturating_sub(w[0].0) as f64 / 1_000_000.0)
        .filter(|d| *d > 0.0)
        .collect();
    if dts.is_empty() {
        return None;
    }
    dts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(dts[dts.len() / 2])
}

/// One unbroken stretch on a uniform grid.
struct PidRun {
    /// (target, actual).
    xy: Vec<(f64, f64)>,
    /// The change of the desired angle per sample, when ATT covers this stretch.
    reference: Option<Vec<f64>>,
}

/// Uniformly sampled runs of (target, actual). Timestamp jitter and a few dropped rows are
/// interpolated; a gap of more than five sample periods ends a run.
fn pid_runs(samples: &[(u64, f64, f64)], att: &[(u64, f64)], dt: f64) -> Vec<PidRun> {
    let mut runs = Vec::new();
    let mut start = 0usize;
    for i in 1..=samples.len() {
        let gap = i == samples.len() || {
            let d = samples[i].0 as f64 / 1e6 - samples[i - 1].0 as f64 / 1e6;
            d <= 0.0 || d > dt * 5.0
        };
        if !gap {
            continue;
        }
        if i - start >= 2 {
            let xy = pid_resample(&samples[start..i], dt);
            let reference = pid_reference(att, samples[start].0 as f64 / 1e6, xy.len(), dt);
            runs.push(PidRun { xy, reference });
        }
        start = i;
    }
    runs
}

fn pid_resample(rows: &[(u64, f64, f64)], dt: f64) -> Vec<(f64, f64)> {
    let t0 = rows[0].0 as f64 / 1e6;
    let span = rows[rows.len() - 1].0 as f64 / 1e6 - t0;
    let n = (span / dt).floor() as usize + 1;
    let mut out = Vec::with_capacity(n);
    let mut j = 0usize;
    for i in 0..n {
        let t = t0 + i as f64 * dt;
        while j + 2 < rows.len() && rows[j + 1].0 as f64 / 1e6 <= t {
            j += 1;
        }
        let ta = rows[j].0 as f64 / 1e6;
        let tb = rows[j + 1].0 as f64 / 1e6;
        let f = if tb > ta { ((t - ta) / (tb - ta)).clamp(0.0, 1.0) } else { 0.0 };
        out.push((rows[j].1 + (rows[j + 1].1 - rows[j].1) * f, rows[j].2 + (rows[j + 1].2 - rows[j].2) * f));
    }
    out
}

fn pid_wrap_deg(delta: f64) -> f64 {
    (delta + 180.0).rem_euclid(360.0) - 180.0
}

/// The desired angle on the grid of one run, as its change per sample. None when ATT does not
/// cover the run at the same rate. A heading that crosses 0° is followed along the short arc.
fn pid_reference(att: &[(u64, f64)], t0: f64, n: usize, dt: f64) -> Option<Vec<f64>> {
    if att.len() < 2 || n < 2 {
        return None;
    }
    let time = |i: usize| att[i].0 as f64 / 1e6;
    let mut j = att.partition_point(|row| row.0 as f64 / 1e6 <= t0).saturating_sub(1).min(att.len() - 2);
    let mut out = Vec::with_capacity(n);
    let mut prev = 0.0;
    for i in 0..n {
        let t = t0 + i as f64 * dt;
        while j + 2 < att.len() && time(j + 1) <= t {
            j += 1;
        }
        let (ta, tb) = (time(j), time(j + 1));
        if tb - ta > dt * 5.0 || t < ta - dt || t > tb + dt {
            return None;
        }
        let f = if tb > ta { ((t - ta) / (tb - ta)).clamp(0.0, 1.0) } else { 0.0 };
        let value = att[j].1 + pid_wrap_deg(att[j + 1].1 - att[j].1) * f;
        out.push(if i == 0 { 0.0 } else { pid_wrap_deg(value - prev) });
        prev = value;
    }
    Some(out)
}

/// Spectra summed over windows, bins 0..=n/2. x is the target, y is the actual, and r is the
/// reference: the desired angle when the log has it, else the target itself.
/// The transfer estimate is (y against r) / (x against r). With r = x that is the usual cross
/// spectrum over the input spectrum. With the desired angle it stays true when the target is
/// partly the outer loop reacting to the vehicle, because wind and vibration are not in r.
#[derive(Clone)]
struct PidSum {
    srr: Vec<f64>,
    sxx: Vec<f64>,
    syy: Vec<f64>,
    num_re: Vec<f64>,
    num_im: Vec<f64>,
    den_re: Vec<f64>,
    den_im: Vec<f64>,
}

/// One frequency of the estimate, or one band of them.
#[derive(Clone, Copy, Default)]
struct PidPoint {
    re: f64,
    im: f64,
    /// The lower of the coherence of r with x and of r with y.
    coherence: f64,
    /// Relative standard error of the estimate; infinite where it is not a measurement.
    error: f64,
}

fn pid_point(srr: f64, sxx: f64, syy: f64, num: (f64, f64), den: (f64, f64), averages: f64) -> PidPoint {
    let den2 = den.0 * den.0 + den.1 * den.1;
    let num2 = num.0 * num.0 + num.1 * num.1;
    if srr <= 1e-30 || sxx <= 1e-30 || syy <= 1e-30 || den2 <= 1e-60 {
        return PidPoint { error: f64::INFINITY, ..Default::default() };
    }
    let coherence = (den2 / (srr * sxx)).min(num2 / (srr * syy)).clamp(0.0, 1.0);
    // Unrelated signals still show a coherence of about one over the number of averages.
    let floor = 1.0 / averages.max(2.0);
    let clean = (coherence - floor) / (1.0 - floor);
    let error = if clean <= 0.0 { f64::INFINITY } else { ((1.0 - clean) / (2.0 * averages * clean)).sqrt() };
    PidPoint {
        re: (num.0 * den.0 + num.1 * den.1) / den2,
        im: (num.1 * den.0 - num.0 * den.1) / den2,
        coherence,
        error,
    }
}

impl PidSum {
    fn new(bins: usize) -> Self {
        let zero = vec![0.0; bins];
        Self {
            srr: zero.clone(),
            sxx: zero.clone(),
            syy: zero.clone(),
            num_re: zero.clone(),
            num_im: zero.clone(),
            den_re: zero.clone(),
            den_im: zero,
        }
    }

    fn add(&mut self, other: &PidSum) {
        for k in 0..self.sxx.len() {
            self.srr[k] += other.srr[k];
            self.sxx[k] += other.sxx[k];
            self.syy[k] += other.syy[k];
            self.num_re[k] += other.num_re[k];
            self.num_im[k] += other.num_im[k];
            self.den_re[k] += other.den_re[k];
            self.den_im[k] += other.den_im[k];
        }
    }

    /// The sums without the windows of `other`.
    fn without(&self, other: &PidSum) -> PidSum {
        let mut out = self.clone();
        for k in 0..out.sxx.len() {
            out.srr[k] -= other.srr[k];
            out.sxx[k] -= other.sxx[k];
            out.syy[k] -= other.syy[k];
            out.num_re[k] -= other.num_re[k];
            out.num_im[k] -= other.num_im[k];
            out.den_re[k] -= other.den_re[k];
            out.den_im[k] -= other.den_im[k];
        }
        out
    }

    /// The estimate over bins `from..to`.
    fn point(&self, from: usize, to: usize, windows: usize) -> PidPoint {
        let sum = |v: &[f64]| v[from..to].iter().sum::<f64>();
        // Neighbouring bins of a Hann window overlap, so a band of them counts for half.
        let averages = windows as f64 * ((to - from) as f64 / 2.0).max(1.0);
        pid_point(
            sum(&self.srr),
            sum(&self.sxx),
            sum(&self.syy),
            (sum(&self.num_re), sum(&self.num_im)),
            (sum(&self.den_re), sum(&self.den_im)),
            averages,
        )
    }
}

struct PidSpectra {
    n: usize,
    /// Windows that entered the estimate.
    windows: usize,
    /// Windows the interval holds.
    windows_total: usize,
    /// RMS of the target over the windows that entered.
    target_rms: f64,
    total: PidSum,
    /// The same windows in up to six blocks in time order, for the standard error of the step response.
    groups: Vec<PidSum>,
}

fn pid_window_count(runs: &[PidRun], n: usize, referenced: bool) -> usize {
    runs.iter()
        .filter(|run| !referenced || run.reference.is_some())
        .map(|run| if run.xy.len() >= n { (run.xy.len() - n) / (n / 2) + 1 } else { 0 })
        .sum()
}

/// Which windows enter the estimate, and the mean square of the target in each. A window where
/// the target moved less than a tenth of the power of the third strongest window is left out:
/// there the target is mostly the outer loop reacting to the vehicle, and it would bend the
/// estimate at the frequencies where a commanded motion has little content.
fn pid_window_choice(runs: &[PidRun], n: usize, referenced: bool) -> (Vec<bool>, Vec<f64>) {
    let mut power = Vec::new();
    for run in runs {
        if referenced && run.reference.is_none() {
            continue;
        }
        let mut from = 0usize;
        while from + n <= run.xy.len() {
            power.push(run.xy[from..from + n].iter().map(|pair| pair.0 * pair.0).sum::<f64>() / n as f64);
            from += n / 2;
        }
    }
    let mut sorted = power.clone();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let bar = sorted.get(2.min(sorted.len().saturating_sub(1))).copied().unwrap_or(0.0) * 0.1;
    (power.iter().map(|p| *p >= bar).collect(), power)
}

/// Hann windows with half overlap. The mean stays in: bin 0 carries the gain at rest, and a Hann
/// window keeps it out of bin 2 and above.
fn pid_spectra(runs: &[PidRun], fs: f64, referenced: bool) -> Option<PidSpectra> {
    let mut n = next_pow2((fs * 2.0) as usize).clamp(64, 4096);
    while pid_window_count(runs, n, referenced) < 4 && n > 64 {
        n /= 2;
    }
    let windows_total = pid_window_count(runs, n, referenced);
    if windows_total < 2 {
        return None;
    }
    let (keep, power) = pid_window_choice(runs, n, referenced);
    let windows = keep.iter().filter(|kept| **kept).count();
    let target_rms = (keep.iter().zip(power.iter()).filter(|(kept, _)| **kept).map(|(_, p)| *p).sum::<f64>() / windows.max(1) as f64).sqrt();
    let half = n / 2;
    let group_count = (windows / 4).clamp(1, 6);
    let mut groups = vec![PidSum::new(half + 1); group_count];
    let hann: Vec<f64> = (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()).collect();
    let mut index = 0usize;
    let mut seen = 0usize;
    for run in runs {
        if referenced && run.reference.is_none() {
            continue;
        }
        let mut from = 0usize;
        while from + n <= run.xy.len() {
            seen += 1;
            if !keep[seen - 1] {
                from += half;
                continue;
            }
            let mut xr = vec![0.0; n];
            let mut xi = vec![0.0; n];
            let mut yr = vec![0.0; n];
            let mut yi = vec![0.0; n];
            for i in 0..n {
                xr[i] = run.xy[from + i].0 * hann[i];
                yr[i] = run.xy[from + i].1 * hann[i];
            }
            fft(&mut xr, &mut xi, false);
            fft(&mut yr, &mut yi, false);
            let (rr, ri) = match run.reference.as_ref().filter(|_| referenced) {
                Some(reference) => {
                    let mut rr: Vec<f64> = (0..n).map(|i| reference[from + i] * hann[i]).collect();
                    let mut ri = vec![0.0; n];
                    fft(&mut rr, &mut ri, false);
                    (rr, ri)
                }
                None => (xr.clone(), xi.clone()),
            };
            let group = &mut groups[(index * group_count / windows).min(group_count - 1)];
            for k in 0..=half {
                group.srr[k] += rr[k] * rr[k] + ri[k] * ri[k];
                group.sxx[k] += xr[k] * xr[k] + xi[k] * xi[k];
                group.syy[k] += yr[k] * yr[k] + yi[k] * yi[k];
                group.num_re[k] += yr[k] * rr[k] + yi[k] * ri[k];
                group.num_im[k] += yi[k] * rr[k] - yr[k] * ri[k];
                group.den_re[k] += xr[k] * rr[k] + xi[k] * ri[k];
                group.den_im[k] += xi[k] * rr[k] - xr[k] * ri[k];
            }
            index += 1;
            from += half;
        }
    }
    let mut total = PidSum::new(half + 1);
    for group in &groups {
        total.add(group);
    }
    Some(PidSpectra { n, windows, windows_total, target_rms, total, groups })
}

/// How much of each bin enters the step response: 1 where the estimate is a measurement,
/// 0 where it is noise, with a ramp between.
fn pid_bin_weights(sum: &PidSum, windows: usize, top_bin: usize) -> Vec<f64> {
    (0..sum.sxx.len()).map(|k| {
        if k > top_bin {
            return 0.0;
        }
        let error = sum.point(k, k + 1, windows).error;
        ((2.0 * PID_ERR_OK - error) / PID_ERR_OK).clamp(0.0, 1.0)
    }).collect()
}

/// The estimate as an impulse response over one window: lag 0 first, negative lags at the end.
fn pid_impulse(sum: &PidSum, weights: &[f64], n: usize) -> Vec<f64> {
    let half = n / 2;
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    for k in 0..=half {
        if weights[k] <= 0.0 {
            continue;
        }
        let point = sum.point(k, k + 1, 2);
        if !point.re.is_finite() || !point.im.is_finite() {
            continue;
        }
        re[k] = point.re * weights[k];
        if k > 0 && k < half {
            im[k] = point.im * weights[k];
            re[n - k] = re[k];
            im[n - k] = -im[k];
        }
    }
    fft(&mut re, &mut im, true);
    re.iter().map(|v| v / n as f64).collect()
}

/// The response to a unit step of the target: the running sum of the impulse response.
fn pid_step_curve(impulse: &[f64], len: usize) -> Vec<f64> {
    let mut acc = 0.0;
    impulse.iter().take(len).map(|v| {
        acc += v;
        acc
    }).collect()
}

/// The share of the impulse response that comes after the target moved. A loop that follows its
/// target is near 1. Near one half, the target was following the vehicle instead.
fn pid_causal_share(impulse: &[f64], len: usize) -> f64 {
    let after: f64 = impulse.iter().take(len).map(|v| v * v).sum();
    let before: f64 = impulse.iter().rev().take(len).map(|v| v * v).sum();
    if after + before <= 1e-30 { 0.0 } else { after / (after + before) }
}

/// One point of the Bode plot: the transfer estimate over a band about a fifth of an octave wide.
struct PidBand {
    hz: f64,
    re: f64,
    im: f64,
    coherence: f64,
    error: f64,
}

impl PidBand {
    fn gain_db(&self) -> f64 {
        let gain = (self.re * self.re + self.im * self.im).sqrt();
        if gain <= 1e-6 { -120.0 } else { 20.0 * gain.log10() }
    }

    fn phase_deg(&self) -> f64 {
        self.im.atan2(self.re).to_degrees()
    }

    fn ok(&self) -> bool {
        self.error <= PID_ERR_OK
    }
}

fn pid_bands(sum: &PidSum, n: usize, fs: f64, windows: usize) -> Vec<PidBand> {
    let half = n / 2;
    let top = (((fs * 0.45).min(PID_BODE_MAX_HZ) * n as f64 / fs) as usize).min(half.saturating_sub(1));
    let mut bands = Vec::new();
    let mut k = 2usize;
    while k <= top {
        let width = ((k as f64 * 0.15) as usize).max(1);
        let end = (k + width).min(top + 1);
        let point = sum.point(k, end, windows);
        bands.push(PidBand {
            hz: fs * (k + end - 1) as f64 / 2.0 / n as f64,
            re: point.re,
            im: point.im,
            coherence: point.coherence,
            error: point.error,
        });
        k = end;
    }
    bands
}

/// Phase in degrees along the bands, each within 180° of the one before.
fn pid_unwrap(phases: impl Iterator<Item = f64>) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    for mut phase in phases {
        if let Some(prev) = out.last() {
            while phase - prev > 180.0 { phase -= 360.0; }
            while prev - phase > 180.0 { phase += 360.0; }
        }
        out.push(phase);
    }
    out
}

/// Keeps the longest run of measured bands and marks every band outside it as not measured.
/// One unmeasured band inside a run does not end it. A lone band that passes far from the run
/// is chance, or the target reacting to vibration, and nothing is read from it.
/// Returns the top of the run in Hz.
fn pid_keep_measured_run(bands: &mut [PidBand]) -> Option<f64> {
    let mut best: Option<(usize, usize, usize)> = None;
    let mut i = 0usize;
    while i < bands.len() {
        if !bands[i].ok() {
            i += 1;
            continue;
        }
        let (start, mut end, mut good) = (i, i, 1usize);
        let mut j = i + 1;
        while j < bands.len() && (bands[j].ok() || (j + 1 < bands.len() && bands[j + 1].ok())) {
            if bands[j].ok() {
                end = j;
                good += 1;
            }
            j += 1;
        }
        if best.is_none_or(|kept| good > kept.2) {
            best = Some((start, end, good));
        }
        i = end + 1;
    }
    let (start, end, _) = best?;
    for (index, band) in bands.iter_mut().enumerate() {
        if index < start || index > end {
            band.error = f64::INFINITY;
        }
    }
    Some(bands[end].hz)
}

/// Why the low end of the estimate is not a loop following its target, if it is not.
/// A rate loop passes a slow target through: gain near 0 dB and little phase.
fn pid_not_following(bands: &[PidBand]) -> Option<String> {
    let low: Vec<&PidBand> = bands.iter().filter(|band| band.ok() && band.hz <= 8.0).take(3).collect();
    let Some(first) = low.first() else {
        return Some("no frequency below 8 Hz is measured: the target barely moved in this interval, or the log holds it only in short stretches.".into());
    };
    let mut gains: Vec<f64> = low.iter().map(|band| band.gain_db()).collect();
    gains.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let gain = gains[gains.len() / 2];
    let phase = first.phase_deg();
    if gain.abs() > 3.0 || phase.abs() > 45.0 {
        return Some(format!(
            "at {:.1} Hz actual is {gain:.1} dB and {phase:.0}° against the target. A loop that follows its target is near 0 dB and 0° there, so in this interval the estimate is not the loop following a commanded motion.",
            first.hz,
        ));
    }
    None
}

fn pid_cross(x0: f64, x1: f64, y0: f64, y1: f64, level: f64) -> f64 {
    if (y1 - y0).abs() < 1e-12 { x0 } else { x0 + (x1 - x0) * (level - y0) / (y1 - y0) }
}

#[derive(Default)]
struct PidClosedLoop {
    low_gain_db: Option<f64>,
    bandwidth_hz: Option<f64>,
    peak_gain_db: Option<f64>,
    peak_hz: Option<f64>,
}

fn pid_closed_loop(bands: &[PidBand]) -> PidClosedLoop {
    let good: Vec<&PidBand> = bands.iter().filter(|band| band.ok()).collect();
    if good.len() < 3 {
        return PidClosedLoop::default();
    }
    let mut low: Vec<f64> = good.iter().take(3).map(|band| band.gain_db()).collect();
    low.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let reference = low[1];
    let mut out = PidClosedLoop { low_gain_db: Some(reference), ..Default::default() };
    for band in &good {
        if out.peak_gain_db.is_none_or(|peak| band.gain_db() > peak) {
            out.peak_gain_db = Some(band.gain_db());
            out.peak_hz = Some(band.hz);
        }
    }
    for pair in bands.windows(2) {
        if !pair[0].ok() || !pair[1].ok() {
            continue;
        }
        let (a, b) = (pair[0].gain_db(), pair[1].gain_db());
        if a > reference - 3.0 && b <= reference - 3.0 {
            out.bandwidth_hz = Some(pid_cross(pair[0].hz.ln(), pair[1].hz.ln(), a, b, reference - 3.0).exp());
            break;
        }
    }
    out
}

#[derive(Default)]
struct PidMargins {
    gain_crossover_hz: Option<f64>,
    phase_margin_deg: Option<f64>,
    phase_crossover_hz: Option<f64>,
    gain_margin_db: Option<f64>,
    sensitivity_peak_db: Option<f64>,
    sensitivity_peak_hz: Option<f64>,
}

/// Margins of the open loop L = T / (1 - T). That identity holds for a loop whose only path from
/// target to output is through the error, so the caller checks the source and the feed-forward gains.
fn pid_margins(bands: &[PidBand]) -> PidMargins {
    let mut out = PidMargins::default();
    let good: Vec<(usize, &PidBand)> = bands.iter().enumerate().filter(|(_, band)| band.error <= PID_ERR_TIGHT).collect();
    if good.len() < 4 {
        return out;
    }
    let open: Vec<(f64, f64)> = good.iter().map(|(_, band)| {
        let (c, d) = (1.0 - band.re, -band.im);
        let den = (c * c + d * d).max(1e-12);
        let re = (band.re * c + band.im * d) / den;
        let im = (band.im * c - band.re * d) / den;
        (20.0 * (re * re + im * im).sqrt().max(1e-6).log10(), im.atan2(re).to_degrees())
    }).collect();
    let mut first = open[0].1;
    if first > 90.0 {
        first -= 360.0;
    }
    let phase = pid_unwrap(std::iter::once(first).chain(open.iter().skip(1).map(|row| row.1)));
    for (i, (_, band)) in good.iter().enumerate() {
        let sensitivity = ((1.0 - band.re).powi(2) + band.im.powi(2)).sqrt();
        let db = 20.0 * sensitivity.max(1e-6).log10();
        if out.sensitivity_peak_db.is_none_or(|peak| db > peak) {
            out.sensitivity_peak_db = Some(db);
            out.sensitivity_peak_hz = Some(band.hz);
        }
        if i == 0 || good[i].0 != good[i - 1].0 + 1 {
            continue;
        }
        let (x0, x1) = (good[i - 1].1.hz.ln(), band.hz.ln());
        let (db0, db1) = (open[i - 1].0, open[i].0);
        if out.gain_crossover_hz.is_none() && db0 >= 0.0 && db1 < 0.0 {
            let x = pid_cross(x0, x1, db0, db1, 0.0);
            let at = phase[i - 1] + (phase[i] - phase[i - 1]) * (x - x0) / (x1 - x0);
            out.gain_crossover_hz = Some(x.exp());
            out.phase_margin_deg = Some(180.0 + at);
        }
        if out.phase_crossover_hz.is_none() && phase[i - 1] > -180.0 && phase[i] <= -180.0 {
            let x = pid_cross(x0, x1, phase[i - 1], phase[i], -180.0);
            let at = db0 + (db1 - db0) * (x - x0) / (x1 - x0);
            out.phase_crossover_hz = Some(x.exp());
            out.gain_margin_db = Some(-at);
        }
    }
    out
}

struct PidStep {
    dt: f64,
    mean: Vec<f64>,
    /// The curve minus and plus its standard error, when the flight gives three blocks or more.
    band: Option<(Vec<f64>, Vec<f64>)>,
}

struct PidStepMetrics {
    steady_state: f64,
    peak: f64,
    peak_time_ms: f64,
    overshoot_pct: f64,
    t50_ms: Option<f64>,
    rise_time_ms: Option<f64>,
    settling_time_ms: Option<f64>,
    spread: Option<f64>,
}

fn pid_first_reach(curve: &[f64], dt: f64, level: f64) -> Option<f64> {
    if curve.first().is_some_and(|v| *v >= level) {
        return Some(0.0);
    }
    for i in 1..curve.len() {
        if curve[i - 1] < level && curve[i] >= level {
            return Some(pid_cross((i - 1) as f64 * dt, i as f64 * dt, curve[i - 1], curve[i], level));
        }
    }
    None
}

/// The numbers of the step response. The target is 1: overshoot is the peak above 1, the rise
/// is from 0.1 to 0.9, and settled is within 0.1 of 1 to the end of the curve.
/// `steady_state` is where the curve sits over its last fifth.
fn pid_step_metrics(step: &PidStep) -> Option<PidStepMetrics> {
    let curve = &step.mean;
    let tail = (curve.len() / 5).max(1);
    let steady = curve[curve.len() - tail..].iter().sum::<f64>() / tail as f64;
    if !(steady.is_finite() && steady > 0.2) {
        return None;
    }
    let (peak_i, peak) = curve.iter().copied().enumerate()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;
    let t10 = pid_first_reach(curve, step.dt, 0.1);
    let t90 = pid_first_reach(curve, step.dt, 0.9);
    let settled = curve.iter().rposition(|v| (v - 1.0).abs() > 0.1)
        .map(|i| i + 1)
        .unwrap_or(0);
    Some(PidStepMetrics {
        steady_state: steady,
        peak,
        peak_time_ms: peak_i as f64 * step.dt * 1000.0,
        overshoot_pct: ((peak - 1.0) * 100.0).max(0.0),
        t50_ms: pid_first_reach(curve, step.dt, 0.5).map(|t| t * 1000.0),
        rise_time_ms: match (t10, t90) {
            (Some(a), Some(b)) if b >= a => Some((b - a) * 1000.0),
            _ => None,
        },
        settling_time_ms: (settled + tail < curve.len()).then_some(settled as f64 * step.dt * 1000.0),
        spread: step.band.as_ref().map(|(low, high)| {
            low.iter().zip(high.iter()).map(|(a, b)| (b - a) / 2.0).sum::<f64>() / low.len().max(1) as f64
        }),
    })
}

/// A curve that the blocks of the flight agree on and that ends near the target. The end of the
/// curve is where a target that partly reacted to the vehicle bends the estimate first.
fn pid_established(m: &PidStepMetrics) -> bool {
    m.spread.is_some_and(|spread| spread <= 0.05) && (m.steady_state - 1.0).abs() <= 0.2
}

struct PidResponse {
    /// The message the pair came from: RATE, or PIDR, PIDP, PIDY.
    source: String,
    target: String,
    actual: String,
    /// The ATT column used as the reference, or None when the target itself was.
    reference: Option<String>,
    count: usize,
    fs: f64,
    window_s: f64,
    windows: usize,
    windows_total: usize,
    target_rms: f64,
    bands: Vec<PidBand>,
    measured_to_hz: Option<f64>,
    /// Why the estimate is not the response of a loop to its target, when it is not.
    /// Then no number and no chart is read from it.
    not_measured: Option<String>,
    step: Option<PidStep>,
    /// Why `step` is absent.
    step_missing: Option<String>,
}

/// The names an estimate reports.
struct PidNames {
    source: String,
    target: String,
    actual: String,
    reference: String,
}

/// The rate loop of one axis that the log holds: where its samples are and how its gains are named.
struct PidLoop<'a> {
    /// `copter`, `quadplane VTOL`, or `fixed wing`.
    controller: &'static str,
    pid_msg: &'static str,
    /// The gain names without their last part, such as `ATC_RAT_RLL_` or `RLL_RATE_`.
    prefix: String,
    pid: &'a [(u64, f64, f64)],
    /// RATE, which belongs to the multicopter loops only.
    rate: &'a [(u64, f64, f64)],
    step_s: f64,
    /// A change of P is proposed from the step response. Not on a fixed wing, where the
    /// feed-forward gain carries most of the response.
    proposes: bool,
}

/// A plane log names its fixed-wing gains RLL_RATE_ and PTCH_RATE_. On a quadplane both sets of
/// loops can be in one log; the one with more samples in the interval is read.
fn pid_loop(scan: &PidScan, axis: usize) -> PidLoop<'_> {
    let row = PID_AXES[axis];
    let plane = scan.parms.keys().any(|name| name.starts_with("RLL_RATE_") || name.starts_with("PTCH_RATE_"));
    if !plane {
        return PidLoop { controller: "copter", pid_msg: row.4, prefix: format!("ATC_RAT_{}_", row.3), pid: &scan.pid[axis], rate: &scan.rate[axis], step_s: PID_STEP_S, proposes: true };
    }
    if scan.piq[axis].len().max(scan.rate[axis].len()) > scan.pid[axis].len() {
        return PidLoop { controller: "quadplane VTOL", pid_msg: row.6, prefix: format!("Q_A_RAT_{}_", row.3), pid: &scan.piq[axis], rate: &scan.rate[axis], step_s: PID_STEP_S, proposes: true };
    }
    PidLoop { controller: "fixed wing", pid_msg: row.4, prefix: row.7.to_string(), pid: &scan.pid[axis], rate: &[], step_s: PID_STEP_WING_S, proposes: false }
}

fn pid_response(scan: &PidScan, axis: usize) -> Result<PidResponse, String> {
    let row = PID_AXES[axis];
    let chosen = pid_loop(scan, axis);
    let (rate, pid) = (chosen.rate, chosen.pid);
    // The PID message carries the target the loop itself subtracts from; RATE's desired is before the target filter.
    let from_pid = pid.len() >= 256 && pid.len() * 5 >= rate.len() * 4;
    let (source, target, actual, samples) = if from_pid { (chosen.pid_msg, "Tar", "Act", pid) } else { ("RATE", row.1, row.2, rate) };
    if samples.len() < 256 {
        return Err(format!(
            "too few samples: {} rows of RATE and {} of {} on the {} axis in this interval. A response needs at least 256.",
            rate.len(), pid.len(), chosen.pid_msg, row.0,
        ));
    }
    let Some(dt) = pid_median_dt(samples) else {
        return Err("sample rate is unknown".into());
    };
    let fs = 1.0 / dt;
    let runs = pid_runs(samples, &scan.att[axis], dt);
    let all: usize = runs.iter().map(|run| run.xy.len()).sum();
    let covered: usize = runs.iter().filter(|run| run.reference.is_some()).map(|run| run.xy.len()).sum();
    let names = PidNames { source: source.to_string(), target: target.to_string(), actual: actual.to_string(), reference: row.5.to_string() };
    let names = &names;
    let step_s = chosen.step_s;
    // Tied to the desired angle when the log has it at this rate. When that holds no response,
    // such as a flight without stick motion, the target itself is tried, and it must pass the
    // same tests of a loop that follows.
    let tied = if covered >= 256 && covered * 5 >= all * 4 { pid_estimate(names, &runs, fs, dt, true, step_s, true) } else { None };
    let tied = match tied {
        Some(response) if response.not_measured.is_none() => return Ok(response),
        other => other,
    };
    match (pid_estimate(names, &runs, fs, dt, false, step_s, true), tied) {
        (Some(direct), _) if direct.not_measured.is_none() => Ok(direct),
        (_, Some(tied)) => Ok(tied),
        (Some(direct), None) => Ok(direct),
        (None, None) => Err(format!(
            "no unbroken stretch of {source} is long enough: {} rows at {fs:.0} Hz. A response needs two windows of {:.1} s.",
            samples.len(), 64.0 / fs,
        )),
    }
}

/// One estimate over the runs: with `referenced`, tied to the desired angle; without, to the target itself.
/// `rate_loop` adds what holds for a rate loop: it passes a slow target through unchanged, and it
/// has a step response worth drawing. Any pair of command and response has to answer after its
/// command, and that is tested either way.
fn pid_estimate(names: &PidNames, runs: &[PidRun], fs: f64, dt: f64, referenced: bool, step_s: f64, rate_loop: bool) -> Option<PidResponse> {
    let source = names.source.as_str();
    let spectra = pid_spectra(runs, fs, referenced)?;
    let n = spectra.n;
    let window_s = n as f64 / fs;
    let count: usize = runs.iter().filter(|run| !referenced || run.reference.is_some()).map(|run| run.xy.len()).sum();
    let mut bands = pid_bands(&spectra.total, n, fs, spectra.windows);
    let measured_to_hz = pid_keep_measured_run(&mut bands);
    let mut not_measured = if rate_loop {
        pid_not_following(&bands)
    } else if measured_to_hz.is_none() {
        Some("no frequency is measured: the command barely moved in this interval, or the log holds it only in short stretches.".to_string())
    } else {
        None
    };
    let len = ((step_s * fs).round() as usize + 1).min(n / 2);
    let top_bin = (measured_to_hz.unwrap_or(0.0) * n as f64 / fs).ceil() as usize;
    let weights = pid_bin_weights(&spectra.total, spectra.windows, top_bin);
    let impulse = pid_impulse(&spectra.total, &weights, n);
    if not_measured.is_none() {
        // A rate loop answers within the step length. Another pair may be slower: a quarter window.
        let causal = pid_causal_share(&impulse, if rate_loop { len.max(4) } else { n / 4 });
        if causal < 0.75 {
            not_measured = Some(format!("only {:.0}% of the estimated response comes after the command moved, so in this interval the estimate is not the loop following a commanded motion.", causal * 100.0));
        }
    }
    let mut step = None;
    let mut step_missing = None;
    if not_measured.is_some() {
        step_missing = not_measured.clone();
    } else if !rate_loop {
        step_missing = Some("no step response is estimated for this pair.".to_string());
    } else if fs < PID_STEP_MIN_HZ {
        step_missing = Some(format!("{source} is logged at {fs:.0} Hz. A step response of {step_s} s needs {PID_STEP_MIN_HZ:.0} Hz or more."));
    } else if window_s < 2.0 * step_s {
        step_missing = Some(format!("{source} is logged at {fs:.0} Hz only in stretches too short for windows longer than {window_s:.2} s. A step response of {step_s} s needs windows of {:.0} s.", 2.0 * step_s));
    } else {
        {
            let mean = pid_step_curve(&impulse, len);
            let band = (spectra.groups.len() >= 3).then(|| {
                // Jackknife: the curve again without each block in turn. How far those curves
                // scatter is the standard error of the curve from all the blocks.
                let curves: Vec<Vec<f64>> = spectra.groups.iter()
                    .map(|group| pid_step_curve(&pid_impulse(&spectra.total.without(group), &weights, n), len))
                    .collect();
                let blocks = curves.len() as f64;
                let mut low = Vec::with_capacity(len);
                let mut high = Vec::with_capacity(len);
                for i in 0..len {
                    let m = curves.iter().map(|c| c[i]).sum::<f64>() / blocks;
                    let error = ((blocks - 1.0) / blocks * curves.iter().map(|c| (c[i] - m).powi(2)).sum::<f64>()).sqrt();
                    low.push(mean[i] - error);
                    high.push(mean[i] + error);
                }
                (low, high)
            });
            step = Some(PidStep { dt, mean, band });
        }
    }
    Some(PidResponse {
        source: names.source.clone(),
        target: names.target.clone(),
        actual: names.actual.clone(),
        reference: referenced.then(|| names.reference.clone()),
        count,
        fs,
        window_s,
        windows: spectra.windows,
        windows_total: spectra.windows_total,
        target_rms: spectra.target_rms,
        bands,
        measured_to_hz,
        not_measured,
        step,
        step_missing,
    })
}

fn pid_r1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn pid_r3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn pid_opt(value: Option<f64>, round: fn(f64) -> f64) -> Value {
    value.filter(|v| v.is_finite()).map(|v| json!(round(v))).unwrap_or(Value::Null)
}

/// Stick steps, the worst tracking moment, and a sustained oscillation, from RATE in deg/s.
#[derive(Default)]
struct PidEvents {
    steps: usize,
    best_step: Option<(u64, u64, f64)>,
    worst_tracking: Option<(u64, f64)>,
    oscillation: Option<(u64, u64, f64)>,
}

/// How long an oscillation of the tracking error has to hold to be reported.
const PID_OSC_HOLD_S: f64 = 0.5;

fn pid_median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sorted.get(sorted.len() / 2).copied().unwrap_or(0.0)
}

fn pid_events(rate_samples: &[(u64, f64, f64)]) -> PidEvents {
    let mut out = PidEvents::default();
    let mut in_step = false;
    let mut step_start_idx = 0usize;
    let mut step_target = 0.0f64;
    let mut step_base = 0.0f64;
    for i in 1..rate_samples.len() {
        let d_delta = (rate_samples[i].1 - rate_samples[i - 1].1).abs();
        let dt = (rate_samples[i].0.saturating_sub(rate_samples[i - 1].0)) as f64 / 1_000_000.0;
        let rate_deriv = if dt > 0.0 { d_delta / dt } else { 0.0 };
        if !in_step && rate_deriv > 100.0 && rate_samples[i].1.abs() > 10.0 {
            in_step = true;
            step_start_idx = i;
            step_base = rate_samples[i - 1].2;
            step_target = rate_samples[i].1;
        } else if in_step {
            let elapsed_ms = (rate_samples[i].0.saturating_sub(rate_samples[step_start_idx].0)) as f64 / 1000.0;
            if elapsed_ms > 400.0 || (rate_samples[i].1 - step_target).abs() > (step_target - step_base).abs() * 0.3 {
                let delta = (step_target - step_base).abs();
                if delta > 15.0 {
                    out.steps += 1;
                    if out.best_step.is_none_or(|best| delta > best.2) {
                        out.best_step = Some((rate_samples[step_start_idx].0, rate_samples[i].0, delta));
                    }
                }
                in_step = false;
            }
        }
    }
    for s in rate_samples {
        let err = (s.1 - s.2).abs();
        if out.worst_tracking.is_none_or(|worst| err > worst.1) {
            out.worst_tracking = Some((s.0, err));
        }
    }
    // An oscillation of the tracking error counts when it holds: windows of 40 samples, each with at
    // least three cycles above 8 deg/s peak to peak between 15 and 65 Hz, back to back for at least
    // PID_OSC_HOLD_S at one frequency (within 20%). One noisy window is not an oscillation.
    let win_size = 40;
    if rate_samples.len() > 60 {
        let mut run: Option<(u64, u64, Vec<f64>)> = None;
        let mut i = 0;
        while i + win_size <= rate_samples.len() {
            let win = &rate_samples[i..i + win_size];
            let dt = (win[win_size - 1].0.saturating_sub(win[0].0)) as f64 / 1_000_000.0;
            let mut hit = None;
            if dt > 0.05 && dt < 0.40 {
                let mut zero_crossings = 0;
                let mut max_amp = 0.0f64;
                let mut min_amp = 0.0f64;
                for j in 1..win.len() {
                    let e0 = win[j - 1].2 - win[j - 1].1;
                    let e1 = win[j].2 - win[j].1;
                    if (e0 > 0.0 && e1 < 0.0) || (e0 < 0.0 && e1 > 0.0) {
                        zero_crossings += 1;
                    }
                    max_amp = max_amp.max(e1);
                    min_amp = min_amp.min(e1);
                }
                let freq = (zero_crossings as f64 / 2.0) / dt;
                if zero_crossings >= 6 && max_amp - min_amp > 8.0 && (15.0..=65.0).contains(&freq) {
                    hit = Some(freq);
                }
            }
            run = match (run, hit) {
                (Some((start, end, mut freqs)), Some(freq))
                    if win[0].0.saturating_sub(end) < 50_000 && (freq / pid_median(&freqs) - 1.0).abs() <= 0.2 =>
                {
                    freqs.push(freq);
                    Some((start, win[win_size - 1].0, freqs))
                }
                (_, Some(freq)) => Some((win[0].0, win[win_size - 1].0, vec![freq])),
                (_, None) => None,
            };
            if let Some((start, end, freqs)) = &run {
                if end.saturating_sub(*start) as f64 / 1e6 >= PID_OSC_HOLD_S {
                    out.oscillation = Some((*start, *end, pid_median(freqs)));
                    break;
                }
            }
            i += win_size;
        }
    }
    out
}

/// The delay that lines actual up with target, from the peak of their cross-correlation.
fn pid_tracking_lag_ms(samples: &[(u64, f64, f64)], dt: f64) -> Option<f64> {
    let n = samples.len().min(4000);
    if n < 100 {
        return None;
    }
    let mut best_lag = 0usize;
    let mut best = f64::NEG_INFINITY;
    for lag in 0..50 {
        let sum: f64 = (0..n - lag).map(|i| samples[i].1 * samples[i + lag].2).sum();
        if sum > best {
            best = sum;
            best_lag = lag;
        }
    }
    (best > 0.0).then_some(best_lag as f64 * dt * 1000.0)
}

/// The logging parameters that set how fast, and how completely, the rate loops reach the log.
const PID_LOG_PARAMS: [&str; 4] = ["LOG_BITMASK", "LOG_FILE_RATEMAX", "LOG_BLK_RATEMAX", "LOG_BACKEND_TYPE"];

/// What the log says about its own logging: the parameters above, the LOG_BITMASK bits that are set,
/// and where the bit list and the logging rates are documented for this vehicle.
fn pid_logging(scan: &PidScan, plane: bool, series: &[(u64, f64, f64)]) -> Value {
    let mut out = serde_json::Map::new();
    // Breaks in the source message: longer than five sample periods, as the estimate's windows end.
    let dts: Vec<u64> = series.windows(2).map(|w| w[1].0.saturating_sub(w[0].0)).filter(|dt| *dt > 0).collect();
    if !dts.is_empty() {
        let mut sorted = dts;
        sorted.sort_unstable();
        let period = sorted[sorted.len() / 2];
        let mut longest = 0u64;
        let mut since = series[0].0;
        let mut gaps = 0usize;
        for w in series.windows(2) {
            if w[1].0.saturating_sub(w[0].0) > period * 5 {
                gaps += 1;
                longest = longest.max(w[0].0.saturating_sub(since));
                since = w[1].0;
            }
        }
        longest = longest.max(series[series.len() - 1].0.saturating_sub(since));
        out.insert("gaps".into(), json!(gaps));
        out.insert("longest_unbroken_s".into(), json!((longest as f64 / 1e5).round() / 10.0));
    }
    if !scan.firmware.is_empty() {
        out.insert("firmware".into(), json!(scan.firmware));
    }
    for name in PID_LOG_PARAMS {
        if let Some(value) = scan.parms.get(name) {
            out.insert(name.to_string(), json!(value));
        }
    }
    if let Some(mask) = scan.parms.get("LOG_BITMASK").map(|v| *v as u64) {
        out.insert("bits_set".into(), json!((0..32).filter(|bit| mask & (1 << bit) != 0).collect::<Vec<u32>>()));
        out.insert("attitude_fast".into(), json!(mask & 1 != 0));
        out.insert("attitude_med".into(), json!(mask & 2 != 0));
        if plane {
            out.insert("attitude_fullrate".into(), json!(mask & (1 << 20) != 0));
        } else {
            out.insert("pid".into(), json!(mask & (1 << 12) != 0));
        }
    }
    let vehicle = if plane { "plane" } else { "copter" };
    out.insert("docs".into(), json!({
        "LOG_BITMASK": format!("https://ardupilot.org/{vehicle}/docs/parameters.html#log-bitmask"),
        "rates": if plane {
            "https://github.com/ArduPilot/ardupilot/blob/master/ArduPlane/Log.cpp"
        } else {
            "https://github.com/ArduPilot/ardupilot/blob/master/ArduCopter/Copter.cpp"
        },
    }));
    Value::Object(out)
}

/// The System ID runs in the log, each with the axis its SID_AXIS excites.
fn pid_system_id_runs(scan: &PidScan) -> Value {
    json!(scan.system_id.iter().map(|&(start, end, axis)| json!({
        "sid_axis": axis,
        "axis": match axis { 1 | 4 | 7 | 10 => "roll", 2 | 5 | 8 | 11 => "pitch", 3 | 6 | 9 | 12 => "yaw", 13 => "throttle", _ => "other" },
        "start_us": start,
        "end_us": if end > 0 { json!(end) } else { Value::Null },
    })).collect::<Vec<_>>())
}

const PID_REVIEW_NOTE: &str = concat!(
    "step_response and frequency_response are one estimate of how actual followed target, averaged over every window of the interval. ",
    "They describe the loop as it flew in this interval. ",
    "reference is the ATT desired angle the estimate is tied to: only motion that was asked for counts, and wind or vibration does not. ",
    "With reference null the target itself is used, and a target that mostly reacted to the vehicle is rejected. ",
    "not_measured, when set, says why this interval holds no response; every number below it is then null, and it is not a finding about the tune. ",
    "step_response.points is the response to a unit step of the target: 1 is the target. ",
    "overshoot_pct is the peak above the target, rise_time_ms is from 0.1 to 0.9, and settling_time_ms is when the curve stays within 0.1 of the target; steady_state is where it sits at the end of the curve. ",
    "windows of windows_total entered the estimate: a window where the target barely moved is left out. ",
    "spread is the standard error of the curve, in units of the step, from leaving out one block of the flight at a time. Above 0.05, or null, or with steady_state more than 0.2 from the target, the curve is not established and proposed_params stays empty. ",
    "The slow end of the curve is the part a gust bends first: a steady_state below 1 in a flight with little stick motion is not a finding about the I gain. ",
    "measured_to_hz is how high the estimate is a measurement. Detail faster than that is not measured. ",
    "gain_db is 0 where actual equals target. A frequency_response row with measured false is not a measurement. ",
    "phase_margin_deg, gain_margin_db, and sensitivity_peak_db are of the open loop L = T/(1-T). ",
    "That holds when the pair is Tar and Act of the PID message and the feed-forward gains are zero; margins_unavailable names the condition that failed. ",
    "A null field was not measured. It is not zero. ",
    "current_params are the gains recorded in the log for this loop; proposed_params is empty when no measured number supports a change, which means this tool found no defect in current_params. ",
    "logging is the log's own logging setup: firmware is the firmware that wrote it, and sample_hz the rate the source message was written at. ",
    "attitude_fast, attitude_med, pid, and attitude_fullrate read LOG_BITMASK with the bit numbers of ArduPilot 4.x: bits 0, 1, and 12 on ArduCopter, bits 0, 1, and 20 on ArduPlane. For another firmware the bit list at docs.LOG_BITMASK is the reference, and bits_set is the raw value. ",
    "In the ArduPilot 4.x source (docs.rates), ArduCopter writes RATE and PIDR, PIDP, PIDY at the main loop rate with bit 0, and at 10 Hz with only bit 1; the PID messages also need bit 12. ArduPlane writes its rate loops at the main loop rate with bit 20, at 25 Hz with bit 0, and at 10 Hz with bit 1. ",
    "A non-zero LOG_FILE_RATEMAX, or LOG_BLK_RATEMAX on block flash (LOG_BACKEND_TYPE 4), caps the rate of each message. ",
    "gaps counts breaks in the source message longer than five sample periods, and longest_unbroken_s is the longest stretch without one. A logger that does not keep up writes gaps, not a slower rate; with many gaps, few windows enter the estimate, and not_measured then says more about the log than about the flight. ",
    "An oscillation in diagnosis held for at least 0.5 s at one frequency. ",
    "system_id_runs are the ArduCopter System ID chirps in the whole log, each with the axis it excited and its TimeUS bounds (end_us null when the log ends during the run). An interval bounded to one run holds that axis's excitation without the rest of the flight.",
);

#[cfg(test)]
pub fn pid_review_bytes(
    bytes: &[u8],
    axis: Option<&str>,
    start_us: Option<u64>,
    end_us: Option<u64>,
) -> Value {
    let Some(scan) = pid_scan(bytes, start_us, end_us) else {
        return json!({ "ok": false, "error": "No message format headers found in log" });
    };
    pid_review_scan(&scan, axis)
}

/// One parse of the log serves every axis and both charts. The last scan is kept, keyed by the
/// file and the interval; calls that arrive together wait for the first one to finish.
fn pid_scan_cached(id: &str, start_us: Option<u64>, end_us: Option<u64>) -> Result<std::sync::Arc<PidScan>, String> {
    type Key = (String, u64, Option<std::time::SystemTime>, Option<u64>, Option<u64>);
    static LAST: std::sync::Mutex<Option<(Key, std::sync::Arc<PidScan>)>> = std::sync::Mutex::new(None);
    let path = resolve(id)?;
    let meta = fs::metadata(&path).map_err(|e| e.to_string())?;
    let key: Key = (id.to_string(), meta.len(), meta.modified().ok(), start_us, end_us);
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((kept, scan)) = last.as_ref() {
        if *kept == key {
            return Ok(scan.clone());
        }
    }
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    let scan = std::sync::Arc::new(pid_scan(&bytes, start_us, end_us).ok_or("No message format headers found in log")?);
    *last = Some((key, scan.clone()));
    Ok(scan)
}

fn pid_review_scan(scan: &PidScan, axis: Option<&str>) -> Value {
    let index = pid_axis_index(axis);
    let (target_axis, des_col, act_col, _, _, _, _, _) = PID_AXES[index];
    let chosen = pid_loop(scan, index);
    let (pid_msg, prefix) = (chosen.pid_msg, chosen.prefix.as_str());
    let rate_samples = chosen.rate;
    let response = match pid_response(scan, index) {
        Ok(response) => response,
        Err(error) => return json!({ "ok": false, "axis": target_axis, "error": error }),
    };
    let measured = response.not_measured.is_none();

    let param = |suffix: &str| scan.parms.get(&format!("{prefix}{suffix}")).copied();
    // A gain is proposed only for a loop where P shapes the step response.
    let p_rule = |value: Option<f64>| value.filter(|_| chosen.proposes);
    let (p_cur, i_cur, d_cur) = (param("P"), param("I"), param("D"));
    let ff_cur = param("FF");
    let dff_cur = param("D_FF");

    let closed = if measured { pid_closed_loop(&response.bands) } else { PidClosedLoop::default() };
    let feed_forward = ff_cur.is_some_and(|v| v.abs() > 1e-9) || dff_cur.is_some_and(|v| v.abs() > 1e-9);
    let margins_unavailable = if !measured {
        response.not_measured.clone()
    } else if response.step.is_none() {
        response.step_missing.clone()
    } else if response.source == "RATE" {
        Some(format!("{pid_msg} is not in this log at the rate of RATE. RATE.{des_col} is the target before the target filter, so T/(1-T) is not the open loop."))
    } else if feed_forward {
        Some(format!("{prefix}FF or {prefix}D_FF is not zero, so part of the output does not pass through the error."))
    } else if !chosen.proposes {
        Some(format!("the {} loop adds feed-forward and a gain that scales with airspeed, so T/(1-T) is not the open loop.", chosen.controller))
    } else {
        None
    };
    let margins = if margins_unavailable.is_none() { pid_margins(&response.bands) } else { PidMargins::default() };
    let step_metrics = response.step.as_ref().and_then(pid_step_metrics);

    let events = pid_events(rate_samples);
    let lag_samples = if rate_samples.len() >= 100 { rate_samples } else { chosen.pid };
    let tracking_lag_ms = pid_median_dt(lag_samples).and_then(|dt| pid_tracking_lag_ms(lag_samples, dt));

    let mut diagnosis = Vec::new();
    let mut recommendations = Vec::new();
    let mut proposed = BTreeMap::new();
    let pm = margins.phase_margin_deg;
    // A change is proposed only from a curve that the blocks of the flight agree on.
    let established = pid_established;
    match (&step_metrics, p_rule(p_cur)) {
        (Some(m), Some(p)) if established(m) && (m.overshoot_pct > 22.0 || pm.is_some_and(|v| v < 32.0)) => {
            let p_prop = (p * 0.85 * 10000.0).round() / 10000.0;
            diagnosis.push(match pm {
                Some(pm) => format!("The {target_axis} rate loop is underdamped: overshoot {:.1}%, phase margin {pm:.1}°.", m.overshoot_pct),
                None => format!("The {target_axis} rate loop is underdamped: overshoot {:.1}%.", m.overshoot_pct),
            });
            recommendations.push(format!("Reduce {prefix}P from {p:.4} to {p_prop:.4}."));
            proposed.insert(format!("{prefix}P"), json!(p_prop));
        }
        (Some(m), Some(p)) if established(m) && pm.is_some_and(|v| v > 52.0) && m.rise_time_ms.is_some_and(|v| v > 90.0) && m.overshoot_pct < 6.0 => {
            let p_prop = (p * 1.20 * 10000.0).round() / 10000.0;
            diagnosis.push(format!(
                "The {target_axis} rate loop is slow with margin to spare: rise time {:.0} ms, overshoot {:.1}%, phase margin {:.1}°.",
                m.rise_time_ms.unwrap_or(0.0), m.overshoot_pct, pm.unwrap_or(0.0),
            ));
            recommendations.push(format!("{prefix}P can rise from {p:.4} to {p_prop:.4}."));
            proposed.insert(format!("{prefix}P"), json!(p_prop));
            for (name, cur) in [("D", d_cur), ("I", i_cur)] {
                if let Some(cur) = cur {
                    proposed.insert(format!("{prefix}{name}"), json!((cur * 1.15 * 10000.0).round() / 10000.0));
                }
            }
        }
        (Some(m), _) if !established(m) => {
            diagnosis.push(format!(
                "The {target_axis} step response is not established (standard error {}, ends at {:.2} of the target), so no gain change follows from its overshoot of {:.1}%.",
                m.spread.map(|v| format!("{v:.2}")).unwrap_or_else(|| "not measured: too few windows".into()), m.steady_state, m.overshoot_pct,
            ));
        }
        (Some(m), _) => {
            diagnosis.push(format!(
                "The {target_axis} rate loop reaches the target in {} ms with {:.1}% overshoot. No measured number supports a gain change.",
                m.rise_time_ms.map(|v| format!("{v:.0}")).unwrap_or_else(|| "an unmeasured".into()), m.overshoot_pct,
            ));
        }
        (None, _) => {
            diagnosis.push(format!(
                "No step response is measured on the {target_axis} axis: {}",
                response.step_missing.clone().unwrap_or_else(|| "the estimate did not settle near a steady value in this interval.".into()),
            ));
        }
    }
    if let Some((_, _, hz)) = events.oscillation {
        diagnosis.push(format!("A sustained {hz:.1} Hz oscillation of the tracking error is on the {target_axis} axis."));
    }

    let mut key_intervals = serde_json::Map::new();
    let best_step_interval = events.best_step.map(|(start, end, delta)| json!({
        "start_us": start.saturating_sub(150_000),
        "end_us": end + 400_000,
        "step_delta_deg_s": pid_r1(delta),
    }));
    if let Some(interval) = &best_step_interval {
        key_intervals.insert("best_step_maneuver".into(), interval.clone());
    }
    if let Some((us, err)) = events.worst_tracking.filter(|row| row.1 > 0.0) {
        key_intervals.insert("worst_tracking".into(), json!({
            "start_us": us.saturating_sub(1_500_000),
            "end_us": us + 1_500_000,
            "max_error_deg_s": pid_r1(err),
        }));
    }
    if let Some((start, end, hz)) = events.oscillation {
        key_intervals.insert("oscillation_event".into(), json!({
            "start_us": start.saturating_sub(500_000),
            "end_us": end + 500_000,
            "frequency_hz": pid_r1(hz),
        }));
    }

    let (chart_msg, chart_des, chart_act) = if rate_samples.is_empty() { (pid_msg, "Tar", "Act") } else { ("RATE", des_col, act_col) };
    let chart_lines = json!([
        { "message": chart_msg, "field": chart_des, "name": "Desired" },
        { "message": chart_msg, "field": chart_act, "name": "Actual" }
    ]);
    let first_us = rate_samples.first().or(chosen.pid.first()).map(|s| s.0).unwrap_or(0);
    let (chart_title, chart_start, chart_end) = if let Some((start, end, hz)) = events.oscillation {
        (format!("{} Control Oscillation ({hz:.1} Hz)", target_axis.to_ascii_uppercase()), start.saturating_sub(500_000), end + 500_000)
    } else if let Some((start, end, _)) = events.best_step {
        (format!("{} Rate Step", target_axis.to_ascii_uppercase()), start.saturating_sub(150_000), end + 400_000)
    } else {
        (format!("{} Tracking Rate", target_axis.to_ascii_uppercase()), first_us, first_us + 5_000_000)
    };

    let step_json = match (&response.step, &step_metrics) {
        (Some(step), Some(m)) => {
            let stride = (step.mean.len() / 25).max(1);
            let points: Vec<Value> = step.mean.iter().enumerate().step_by(stride)
                .map(|(i, v)| json!({ "t_ms": pid_r1(i as f64 * step.dt * 1000.0), "value": pid_r3(*v) }))
                .collect();
            json!({
                "steady_state": pid_r3(m.steady_state),
                "peak": pid_r3(m.peak),
                "peak_time_ms": pid_r1(m.peak_time_ms),
                "overshoot_pct": pid_r1(m.overshoot_pct),
                "t50_ms": pid_opt(m.t50_ms, pid_r1),
                "rise_time_ms": pid_opt(m.rise_time_ms, pid_r1),
                "settling_time_ms": pid_opt(m.settling_time_ms, pid_r1),
                "spread": pid_opt(m.spread, pid_r3),
                "points": points,
            })
        }
        _ => Value::Null,
    };
    let frequency_response: Vec<Value> = if measured {
        let stride = response.bands.len().div_ceil(24).max(1);
        let phases = pid_unwrap(response.bands.iter().map(PidBand::phase_deg));
        response.bands.iter().zip(phases.iter()).step_by(stride).map(|(band, phase)| json!({
            "hz": pid_r1(band.hz),
            "gain_db": pid_r1(band.gain_db()),
            "phase_deg": pid_r1(*phase),
            "coherence": (band.coherence * 100.0).round() / 100.0,
            "measured": band.ok(),
        })).collect()
    } else {
        Vec::new()
    };

    let mut current = serde_json::Map::new();
    for (suffix, value) in [("P", p_cur), ("I", i_cur), ("D", d_cur), ("FF", ff_cur), ("D_FF", dff_cur)] {
        if let Some(value) = value {
            current.insert(format!("{prefix}{suffix}"), json!(value));
        }
    }

    json!({
        "ok": true,
        "axis": target_axis,
        "controller": chosen.controller,
        "source": response.source,
        "target": response.target,
        "actual": response.actual,
        "reference": response.reference.as_ref().map(|column| format!("ATT.{column}")),
        "samples_analyzed": response.count,
        "sample_hz": pid_r1(response.fs),
        "logging": pid_logging(scan, chosen.controller != "copter", if response.source == "RATE" { chosen.rate } else { chosen.pid }),
        "system_id_runs": pid_system_id_runs(scan),
        "window_s": pid_r3(response.window_s),
        "windows": response.windows,
        "windows_total": response.windows_total,
        "target_rms": pid_r3(response.target_rms),
        "not_measured": response.not_measured,
        "measured_to_hz": if measured { pid_opt(response.measured_to_hz, pid_r1) } else { Value::Null },
        "step_response": step_json,
        "step_response_unavailable": if step_metrics.is_some() { Value::Null } else {
            json!(response.step_missing.clone().unwrap_or_else(|| "the estimated response did not settle near a steady value".into()))
        },
        "frequency_response": frequency_response,
        "metrics": {
            "rise_time_ms": pid_opt(step_metrics.as_ref().and_then(|m| m.rise_time_ms), pid_r1),
            "overshoot_pct": pid_opt(step_metrics.as_ref().map(|m| m.overshoot_pct), pid_r1),
            "settling_time_ms": pid_opt(step_metrics.as_ref().and_then(|m| m.settling_time_ms), pid_r1),
            "bandwidth_hz": pid_opt(closed.bandwidth_hz, pid_r1),
            "low_gain_db": pid_opt(closed.low_gain_db, pid_r1),
            "peak_gain_db": pid_opt(closed.peak_gain_db, pid_r1),
            "peak_hz": pid_opt(closed.peak_hz, pid_r1),
            "phase_margin_deg": pid_opt(margins.phase_margin_deg, pid_r1),
            "gain_crossover_hz": pid_opt(margins.gain_crossover_hz, pid_r1),
            "gain_margin_db": pid_opt(margins.gain_margin_db, pid_r1),
            "phase_crossover_hz": pid_opt(margins.phase_crossover_hz, pid_r1),
            "sensitivity_peak_db": pid_opt(margins.sensitivity_peak_db, pid_r1),
            "sensitivity_peak_hz": pid_opt(margins.sensitivity_peak_hz, pid_r1),
            "tracking_lag_ms": pid_opt(tracking_lag_ms, pid_r1),
        },
        "margins_unavailable": margins_unavailable,
        "step_maneuvers_detected": events.steps,
        "key_intervals": Value::Object(key_intervals),
        "best_step_interval": best_step_interval,
        "suggested_chart": {
            "title": chart_title,
            "start_us": chart_start,
            "end_us": chart_end,
            "axes": [{ "label": if chart_msg == "RATE" { "deg/s" } else { "rate" }, "lines": chart_lines }],
        },
        "current_params": Value::Object(current),
        "proposed_params": Value::Object(proposed.into_iter().collect()),
        "diagnosis": diagnosis,
        "recommendations": recommendations,
        "note": PID_REVIEW_NOTE,
    })
}

pub fn pid_review(
    id: &str,
    axis: Option<&str>,
    start_us: Option<u64>,
    end_us: Option<u64>,
) -> Value {
    match pid_scan_cached(id, start_us, end_us) {
        Ok(scan) => pid_review_scan(&scan, axis),
        Err(error) => json!({ "ok": false, "error": error }),
    }
}

/// The chart for `show_pid_response`: `kind` is `step` or `bode`, `axis` is `all` or one axis.
pub fn pid_picture(id: &str, spec: &Value) -> Value {
    if id.is_empty() {
        return json!({ "ok": false, "error": "file is required. file is an id from list_logs, or local.id from vehicle_logs." });
    }
    let (start_us, end_us) = pid_spec_interval(spec);
    match pid_scan_cached(id, start_us, end_us) {
        Ok(scan) => pid_picture_scan(&scan, spec),
        Err(error) => json!({ "ok": false, "error": error }),
    }
}

fn pid_spec_interval(spec: &Value) -> (Option<u64>, Option<u64>) {
    (spec.get("start_us").and_then(Value::as_u64), spec.get("end_us").and_then(Value::as_u64))
}

#[cfg(test)]
fn pid_picture_bytes(bytes: &[u8], spec: &Value) -> Value {
    let (start_us, end_us) = pid_spec_interval(spec);
    match pid_scan(bytes, start_us, end_us) {
        Some(scan) => pid_picture_scan(&scan, spec),
        None => json!({ "ok": false, "error": "No message format headers found in log" }),
    }
}

fn pid_picture_scan(scan: &PidScan, spec: &Value) -> Value {
    let kind = spec.get("kind").and_then(Value::as_str).unwrap_or("step").to_ascii_lowercase();
    if kind != "step" && kind != "bode" {
        return json!({ "ok": false, "error": "kind is step or bode." });
    }
    let axis = spec.get("axis").and_then(Value::as_str).unwrap_or("all").to_ascii_lowercase();
    let wanted: Vec<usize> = match PID_AXES.iter().position(|row| row.0 == axis) {
        Some(index) => vec![index],
        None if axis == "all" => vec![0, 1, 2],
        None => return json!({ "ok": false, "error": "axis is all, roll, pitch, or yaw." }),
    };
    let mut left = Vec::new();
    let mut under = Vec::new();
    let mut right = Vec::new();
    let mut drawn = Vec::new();
    let mut missing = Vec::new();
    let mut x_end = 0.0f64;
    let mut hz_span = (f64::INFINITY, 0.0f64);
    let mut phase_span = (0.0f64, 0.0f64);
    for index in wanted {
        let name = PID_AXES[index].0;
        let response = match pid_response(scan, index) {
            Ok(response) => response,
            Err(error) => {
                missing.push(json!({ "axis": name, "reason": error }));
                continue;
            }
        };
        if let Some(reason) = &response.not_measured {
            missing.push(json!({ "axis": name, "reason": reason }));
            continue;
        }
        let mut row = json!({
            "axis": name,
            "controller": pid_loop(scan, index).controller,
            "source": response.source,
            "reference": response.reference.as_ref().map(|column| format!("ATT.{column}")),
            "sample_hz": pid_r1(response.fs),
            "windows": response.windows,
            "measured_to_hz": pid_opt(response.measured_to_hz, pid_r1),
        });
        if kind == "step" {
            let Some(step) = &response.step else {
                missing.push(json!({ "axis": name, "reason": response.step_missing }));
                continue;
            };
            let x: Vec<f64> = (0..step.mean.len()).map(|i| pid_r1(i as f64 * step.dt * 1000.0)).collect();
            x_end = x_end.max(x.last().copied().unwrap_or(0.0));
            let y = |curve: &[f64]| curve.iter().map(|v| pid_r3(*v)).collect::<Vec<f64>>();
            if let Some((low, high)) = &step.band {
                under.push(json!({ "name": format!("{name} −1σ"), "x": x, "y": y(low), "hue": index, "quiet": true }));
                under.push(json!({ "name": format!("{name} +1σ"), "x": x, "y": y(high), "hue": index, "quiet": true, "fill": true }));
            }
            let metrics = pid_step_metrics(step);
            // The legend carries the two numbers the curve is read for. A curve that is not
            // established is dashed.
            let mut label = name.to_string();
            let mut solid = false;
            if let Some(m) = &metrics {
                label.push_str(&format!(" · +{:.0} %", m.overshoot_pct));
                if let Some(rise) = m.rise_time_ms {
                    label.push_str(&format!(" · {rise:.0} ms"));
                }
                solid = pid_established(m);
            }
            let mut line = json!({ "name": label, "x": x, "y": y(&step.mean), "hue": index });
            if !solid {
                line["dash"] = json!("dash");
            }
            left.push(line);
            row["established"] = json!(solid);
            if let Some(m) = metrics {
                row["steady_state"] = json!(pid_r3(m.steady_state));
                row["peak"] = json!(pid_r3(m.peak));
                row["overshoot_pct"] = json!(pid_r1(m.overshoot_pct));
                row["rise_time_ms"] = pid_opt(m.rise_time_ms, pid_r1);
                row["settling_time_ms"] = pid_opt(m.settling_time_ms, pid_r1);
                row["spread"] = pid_opt(m.spread, pid_r3);
            }
        } else {
            let phases = pid_unwrap(response.bands.iter().map(PidBand::phase_deg));
            let mut x = Vec::new();
            let mut gain = Vec::new();
            let mut phase = Vec::new();
            // Only the measured run is drawn. A band inside it that is not a measurement is a gap.
            let first = response.bands.iter().position(PidBand::ok);
            let last = response.bands.iter().rposition(PidBand::ok);
            let (Some(first), Some(last)) = (first, last) else {
                missing.push(json!({ "axis": name, "reason": "fewer than two frequencies are measured in this interval." }));
                continue;
            };
            for (band, deg) in response.bands.iter().zip(phases.iter()).skip(first).take(last + 1 - first) {
                x.push(json!(pid_r3(band.hz)));
                if band.ok() {
                    gain.push(json!(pid_r1(band.gain_db())));
                    phase.push(json!(pid_r1(*deg)));
                    hz_span = (hz_span.0.min(band.hz), hz_span.1.max(band.hz));
                    phase_span = (phase_span.0.min(*deg), phase_span.1.max(*deg));
                } else {
                    gain.push(Value::Null);
                    phase.push(Value::Null);
                }
            }
            if gain.iter().filter(|v| !v.is_null()).count() < 2 {
                missing.push(json!({ "axis": name, "reason": "fewer than two frequencies are measured in this interval." }));
                continue;
            }
            let closed = pid_closed_loop(&response.bands);
            // The legend carries the resonance, when the gain rises above its low-frequency
            // level, and the bandwidth.
            let mut label = name.to_string();
            if let (Some(peak), Some(hz), Some(low)) = (closed.peak_gain_db, closed.peak_hz, closed.low_gain_db) {
                if peak > low + 0.5 {
                    label.push_str(&format!(" · peak {peak:+.1} dB at {hz:.1} Hz"));
                }
            }
            if let Some(hz) = closed.bandwidth_hz {
                label.push_str(&format!(" · −3 dB at {hz:.1} Hz"));
            }
            left.push(json!({ "name": label, "x": x, "y": gain, "hue": index }));
            right.push(json!({ "name": format!("{name} phase"), "x": x, "y": phase, "hue": index, "unlisted": true }));
            row["low_gain_db"] = pid_opt(closed.low_gain_db, pid_r1);
            row["bandwidth_hz"] = pid_opt(closed.bandwidth_hz, pid_r1);
            row["peak_gain_db"] = pid_opt(closed.peak_gain_db, pid_r1);
            row["peak_hz"] = pid_opt(closed.peak_hz, pid_r1);
        }
        drawn.push(row);
    }
    if drawn.is_empty() {
        return json!({
            "ok": false,
            "error": "this interval holds no measured response, so no chart is drawn.",
            "missing": missing,
            "note": "Each reason in missing is about what the log holds. It is not a finding about the tune.",
        });
    }
    let asked = spec.get("title").and_then(Value::as_str).unwrap_or("").trim();
    let (chart, note) = if kind == "step" {
        let mut lines = vec![
            json!({ "name": "target −10 %", "x": [0.0, x_end], "y": [0.9, 0.9], "hue": 2, "quiet": true }),
            json!({ "name": "target +10 %", "x": [0.0, x_end], "y": [1.1, 1.1], "hue": 2, "quiet": true, "fill": true }),
        ];
        lines.append(&mut under);
        lines.push(json!({ "name": "target", "x": [0.0, x_end], "y": [1.0, 1.0], "hue": 2, "dash": "dot", "faint": true }));
        lines.append(&mut left);
        let left = lines;
        let title = if asked.is_empty() { "rate step response".to_string() } else { asked.to_string() };
        (json!({
            "title": title,
            "x_label": "ms",
            "axes": [{ "label": "response to a unit step", "lines": left }],
        }), concat!(
            "The chart is drawn under the reply. ",
            "x is milliseconds after a unit step of the rate target. 1, the dotted line, is the target, and the grey band is 0.9 to 1.1, the band settling_time_ms uses. ",
            "Each line is estimated from the whole interval, not one manoeuvre. ",
            "The legend of each axis carries its overshoot and its rise time. ",
            "The shaded band around a line is the standard error of the curve, from leaving out one block of the flight at a time; a line without one had too few windows for it. ",
            "A dashed line has established false: its band is wide or it ends far from the target, and its overshoot is not a finding about the tune. ",
            "measured_to_hz is how high the estimate is a measurement; detail faster than that is not measured. ",
            "An axis in missing was not drawn, with the reason. That reason is about what the log holds, not about the tune. ",
            "The samples themselves are not in this text.",
        ))
    } else {
        let title = if asked.is_empty() { "rate loop frequency response".to_string() } else { asked.to_string() };
        let ends = [pid_r3(hz_span.0), pid_r3(hz_span.1)];
        let mut lines = vec![
            json!({ "name": "−3 dB", "x": ends, "y": [-3.0, -3.0], "hue": 2, "quiet": true }),
            json!({ "name": "+3 dB", "x": ends, "y": [3.0, 3.0], "hue": 2, "quiet": true, "fill": true }),
            json!({ "name": "target", "x": ends, "y": [0.0, 0.0], "hue": 2, "dash": "dot", "faint": true }),
        ];
        lines.append(&mut left);
        let phase_low = (phase_span.0 / 45.0).floor() * 45.0;
        let phase_high = ((phase_span.1 / 45.0).ceil() * 45.0).max(0.0);
        (json!({
            "title": title,
            "x_label": "Hz",
            "x_log": true,
            "stack": true,
            "axes": [
                { "label": "gain, dB", "lines": lines },
                { "label": "phase, °", "lines": right, "range": [phase_low, phase_high], "dtick": 45 },
            ],
        }), concat!(
            "The chart is drawn under the reply. ",
            "It is the closed loop, actual over target, in two panels over one frequency axis in Hz on a log scale: gain in dB above, phase in degrees below, one colour per axis. ",
            "0 dB, the dotted line, is actual equal to target, and the grey band is −3 to +3 dB. ",
            "The legend of each axis carries its resonance, peak_gain_db at peak_hz, when the gain rises above low_gain_db, and bandwidth_hz, where the gain falls 3 dB below low_gain_db. ",
            "A line ends where the measurement ends. ",
            "A gap in a line is a band that is not a measurement. ",
            "This is not the open loop: a phase margin or a gain margin is not read off this chart. ",
            "An axis in missing was not drawn, with the reason. That reason is about what the log holds, not about the tune. ",
            "The samples themselves are not in this text.",
        ))
    };
    let mut out = json!({
        "ok": true,
        "kind": kind,
        "title": chart["title"],
        "chart": chart,
        "axes": drawn,
        "missing": missing,
        "note": note,
    });
    if let Some(slug) = chart_slug(spec) {
        out["slug"] = json!(slug);
        out["chart"]["slug"] = json!(slug);
    }
    out
}
