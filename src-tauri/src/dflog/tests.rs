// Tests.


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_float_in_a_log_row_rounds_to_four_places() {
        let ugly = round_logged(&json!(0.1f32));
        assert_eq!(ugly.to_string(), "0.1", "{ugly}");
        assert_eq!(round_logged(&json!(1.23456789_f64)).to_string(), "1.2346");
        assert_eq!(round_logged(&json!(1_500_000_u64)), json!(1_500_000));
        assert_eq!(round_logged(&json!(-0.30000004_f64)).to_string(), "-0.3");
    }

    #[test]
    fn a_stored_name_carries_the_content_hash() {
        let id = "002F00354D53501220303932/5262f54c6046823c_20260930090738_1.bin";
        let parts = stored_name_parts(id);
        assert_eq!(parts["sha256_16"], json!("5262f54c6046823c"));
        assert_eq!(parts["flight_stamp"], json!("20260930090738"));
        assert_eq!(parts["vehicle"], json!("002F00354D53501220303932"));
        assert!(parts.get("onboard_id").is_none());
        let older = stored_name_parts("5262f54c6046823c-3.bin");
        assert_eq!(older["sha256_16"], json!("5262f54c6046823c"));
        assert!(stored_name_parts("log-3.bin").get("sha256_16").is_none());
    }

    fn push_fmt(buf: &mut Vec<u8>, type_id: u8, name: &str, format: &str, labels: &str) {
        let width: usize = format.chars().map(|c| field_size(c).unwrap()).sum();
        buf.extend_from_slice(&[HEAD1, HEAD2, FMT_TYPE, type_id, (width + 3) as u8]);
        let mut name_b = [0u8; 4];
        name_b[..name.len()].copy_from_slice(name.as_bytes());
        buf.extend_from_slice(&name_b);
        let mut fmt_b = [0u8; 16];
        fmt_b[..format.len()].copy_from_slice(format.as_bytes());
        buf.extend_from_slice(&fmt_b);
        let mut lab = [0u8; 64];
        lab[..labels.len()].copy_from_slice(labels.as_bytes());
        buf.extend_from_slice(&lab);
    }

    fn push_dsf(buf: &mut Vec<u8>, type_id: u8, time: u64, dp: u32, fmn: u32) {
        buf.extend_from_slice(&[HEAD1, HEAD2, type_id]);
        buf.extend_from_slice(&time.to_le_bytes());
        buf.extend_from_slice(&dp.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&100u32.to_le_bytes());
        buf.extend_from_slice(&fmn.to_le_bytes());
        buf.extend_from_slice(&800u32.to_le_bytes());
        buf.extend_from_slice(&400u32.to_le_bytes());
    }

    #[test]
    fn dsf_reports_the_rejected_write_increase_and_the_tightest_buffer() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 170, "DSF", "QIHIIII", "TimeUS,Dp,Blk,Bytes,FMn,FMx,FAv");
        push_dsf(&mut buf, 170, 1_000_000, 2, 500);
        push_dsf(&mut buf, 170, 2_000_000, 5, 40);
        let logging = logging_of(&buf);
        assert_eq!(logging["rows"], json!(2));
        assert_eq!(logging["dp_first"], json!(2));
        assert_eq!(logging["dp_last"], json!(5));
        assert_eq!(logging["dp_increase"], json!(3));
        assert_eq!(logging["fmn_min"], json!(40));
        let note = logging["note"].as_str().unwrap();
        assert!(note.contains("It does not name which message was rejected."));
        assert!(note.contains("It does not say the storage failed."));
        assert!(!note.contains("LOG_FILE_BUFSIZE"));
    }

    #[test]
    fn a_missing_dsf_message_is_unknown_not_zero_drops() {
        let logging = logging_of(&[]);
        assert_eq!(logging["rows"], json!(0));
        assert!(logging.get("dp_increase").is_none());
        let note = logging["note"].as_str().unwrap();
        assert!(note.contains("Rejected writes are unknown."));
        assert!(note.contains("That is not a count of zero."));
    }

    #[test]
    fn a_falling_dp_counter_has_no_increase() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 170, "DSF", "QIHIIII", "TimeUS,Dp,Blk,Bytes,FMn,FMx,FAv");
        push_dsf(&mut buf, 170, 1_000_000, 5, 100);
        push_dsf(&mut buf, 170, 2_000_000, 1, 100);
        let logging = logging_of(&buf);
        assert_eq!(logging["dp_first"], json!(5));
        assert_eq!(logging["dp_last"], json!(1));
        assert!(logging["dp_increase"].is_null());
        assert!(logging["note"].as_str().unwrap().contains("the counter fell"));
    }

    #[test]
    fn decodes_fmt_and_a_sample() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 129, "RATE", "Qf", "TimeUS,P");
        buf.extend_from_slice(&[HEAD1, HEAD2, 129]);
        buf.extend_from_slice(&1_000_000u64.to_le_bytes());
        buf.extend_from_slice(&0.25f32.to_le_bytes());
        let formats = parse_formats(&buf);
        assert_eq!(formats[&129].name, "RATE");
        let mut rows = Vec::new();
        let mut incomplete = false;
        walk(&buf, &formats, |fmt, _, values| {
            if fmt.name == "RATE" {
                rows.push(values.to_vec());
            }
        }, &mut incomplete);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0].as_u64(), Some(1_000_000));
        assert!((rows[0][1].as_f64().unwrap() - 0.25).abs() < 1e-6);
    }

    fn push_qz(buf: &mut Vec<u8>, type_id: u8, time: u64, text: &str) {
        buf.extend_from_slice(&[HEAD1, HEAD2, type_id]);
        buf.extend_from_slice(&time.to_le_bytes());
        let mut msg = [0u8; 64];
        msg[..text.len()].copy_from_slice(text.as_bytes());
        buf.extend_from_slice(&msg);
    }

    #[test]
    fn summarizes_firmware_frame_and_duration_from_head_and_tail() {
        let mut head = Vec::new();
        push_fmt(&mut head, 160, "MSG", "QZ", "TimeUS,Message");
        push_fmt(&mut head, 161, "GPS", "QBIH", "TimeUS,Status,GMS,GWk");
        push_qz(&mut head, 160, 1_000_000, "ArduCopter V4.7.1 (dbe79216)");
        push_qz(&mut head, 160, 2_000_000, "Frame: QUAD/X");
        head.extend_from_slice(&[HEAD1, HEAD2, 161]);
        head.extend_from_slice(&3_000_000u64.to_le_bytes());
        head.push(3);
        head.extend_from_slice(&0u32.to_le_bytes());
        head.extend_from_slice(&2380u16.to_le_bytes());
        let mut tail = Vec::new();
        push_qz(&mut tail, 160, 61_000_000, "Disarmed");
        let summary = summarize_log(&head, &tail);
        assert_eq!(summary.firmware, "ArduCopter V4.7.1 (dbe79216)");
        assert_eq!(summary.frame, "QUAD/X");
        assert_eq!(summary.duration_us, 60_000_000);
        assert!(summary.start_utc.ends_with('Z'), "{}", summary.start_utc);
        push_qz(&mut head, 160, u64::MAX, "not a time");
        let bounded = summarize_log(&head, &tail);
        assert_eq!(bounded.duration_us, 60_000_000, "a TimeUS of u64::MAX is not a longer flight");
    }

    #[test]
    fn fft_finds_a_known_tone() {
        let mut samples = Vec::new();
        let rate = 100.0;
        for i in 0..256 {
            let t = i as f64 / rate;
            let y = (2.0 * std::f64::consts::PI * 10.0 * t).sin();
            samples.push(((i as f64 * 1_000_000.0 / rate) as u64, y));
        }
        let out = fft_peak(&samples, false);
        let hz = out["peak_hz"].as_f64().unwrap();
        assert!((hz - 10.0).abs() < 1.0, "{out}");
    }

    #[test]
    fn gyro_batch_peak_ignores_aircraft_motion() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 248, "ISBH", "QHBBHHQf", "TimeUS,N,type,instance,mul,smp_cnt,SampleUS,smp_rate");
        push_fmt(&mut buf, 249, "ISBD", "QHHaaa", "TimeUS,N,seqno,x,y,z");
        let mul = 100.0;
        let rate = 1000.0_f32;
        let count = 256u16;
        let mut head = Vec::new();
        head.extend_from_slice(&0u64.to_le_bytes());
        head.extend_from_slice(&1u16.to_le_bytes());
        head.push(1);
        head.push(0);
        head.extend_from_slice(&100u16.to_le_bytes());
        head.extend_from_slice(&count.to_le_bytes());
        head.extend_from_slice(&0u64.to_le_bytes());
        head.extend_from_slice(&rate.to_le_bytes());
        buf.extend_from_slice(&[HEAD1, HEAD2, 248]);
        buf.extend_from_slice(&head);
        for seq in 0..8u16 {
            let mut body = Vec::new();
            body.extend_from_slice(&0u64.to_le_bytes());
            body.extend_from_slice(&1u16.to_le_bytes());
            body.extend_from_slice(&seq.to_le_bytes());
            for axis in 0..3 {
                for k in 0..32u16 {
                    let i = seq as f64 * 32.0 + k as f64;
                    let t = i / 1000.0;
                    let sample = if axis == 0 {
                        5.0 * (2.0 * std::f64::consts::PI * t).sin()
                            + 0.4 * (2.0 * std::f64::consts::PI * 80.0 * t).sin()
                            + 0.2 * (2.0 * std::f64::consts::PI * 160.0 * t).sin()
                    } else {
                        0.0
                    };
                    body.extend_from_slice(&((sample * mul).round() as i16).to_le_bytes());
                }
            }
            buf.extend_from_slice(&[HEAD1, HEAD2, 249]);
            buf.extend_from_slice(&body);
        }
        push_fmt(&mut buf, 140, "PARM", "QNf", "TimeUS,Name,Value");
        let parm = |buf: &mut Vec<u8>, name: &str, value: f32| {
            buf.extend_from_slice(&[HEAD1, HEAD2, 140]);
            buf.extend_from_slice(&0u64.to_le_bytes());
            let mut bytes = [0u8; 16];
            bytes[..name.len()].copy_from_slice(name.as_bytes());
            buf.extend_from_slice(&bytes);
            buf.extend_from_slice(&value.to_le_bytes());
        };
        parm(&mut buf, "INS_HNTCH_ENABLE", 1.0);
        parm(&mut buf, "INS_HNTCH_FREQ", 80.0);
        parm(&mut buf, "INS_HNTCH_BW", 40.0);
        parm(&mut buf, "INS_HNTCH_ATT", 40.0);
        parm(&mut buf, "INS_HNTCH_HMNCS", 1.0);
        parm(&mut buf, "INS_HNTCH_MODE", 0.0);
        let out = gyro_batch_fft_bytes(&buf);
        let hz = out["axes"][0]["peak_hz"].as_f64().unwrap();
        assert!((hz - 80.0).abs() < 2.0, "{out}");
        let second = out["harmonics"].as_array().unwrap().iter().find(|row| row["n"].as_u64() == Some(2)).unwrap();
        let db = second["db"].as_f64().unwrap();
        assert!((db + 6.0).abs() < 6.0, "{out}");
        assert_eq!(out["spectrum"].as_array().unwrap().len(), 32);
        let column = &out["spectrogram"]["columns"][0];
        assert!((column["hz"].as_f64().unwrap() - 80.0).abs() < 2.0, "{out}");
        let spectrum = picture_bytes(&buf, "spectrum", &json!({ "title": "hover", "slug": "gyro" }));
        assert_eq!(spectrum["ok"], json!(true), "{spectrum}");
        assert_eq!(spectrum["chart"]["x_label"], json!("Hz"));
        assert_eq!(spectrum["chart"]["axes"][0]["lines"][0]["name"], json!("X"));
        let lines = spectrum["chart"]["axes"][0]["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 3, "{spectrum}");
        assert_eq!(lines[1]["name"], json!("Y"));
        assert_eq!(lines[2]["name"], json!("Z"));
        let bins = lines[0]["x"].as_array().unwrap();
        assert!(bins.len() > 32, "{spectrum}");
        let loud = lines[0]["y"].as_array().unwrap().iter().filter_map(|value| value.as_f64()).fold(f64::NEG_INFINITY, f64::max);
        let quiet = lines[1]["y"].as_array().unwrap().iter().filter_map(|value| value.as_f64()).fold(f64::NEG_INFINITY, f64::max);
        // Absolute dB as WebTools shows it: the loud axis stands far above the quiet one.
        assert!(loud - quiet > 40.0, "{spectrum}");
        // The filters' gain on its own axis: deep at the 80 Hz notch, flat far from it (no low-pass in this log).
        let right = spectrum["chart"]["axes"][1]["lines"].as_array().unwrap();
        let notch = right.iter().find(|line| line["name"] == "filters").unwrap();
        // A static notch at 80 Hz with only the 1st harmonic: 80 Hz notched, 160 Hz and up not.
        let marks = right.iter().find(|line| line["name"] == "harmonic, notched").unwrap();
        assert_eq!(marks["x"][0], json!(80.0), "{marks}");
        let open = right.iter().find(|line| line["name"] == "harmonic, not notched").unwrap();
        assert_eq!(open["x"][0], json!(160.0), "{open}");
        assert_eq!(notch["dash"], json!("dot"));
        let at = |want: f64| notch["x"].as_array().unwrap().iter().zip(notch["y"].as_array().unwrap())
            .min_by(|a, b| (a.0.as_f64().unwrap() - want).abs().partial_cmp(&(b.0.as_f64().unwrap() - want).abs()).unwrap())
            .map(|(_, y)| y.as_f64().unwrap()).unwrap();
        assert!(at(80.0) < -25.0, "{notch}");
        assert!(at(300.0) > -1.0, "{notch}");
        assert_eq!(spectrum["slug"], json!("gyro"));
        let map = picture_bytes(&buf, "spectrogram", &json!({}));
        assert_eq!(map["chart"]["x_label"], json!("s"), "{map}");
        assert_eq!(map["title"], json!("spectrogram"));
        let hz = map["chart"]["heatmap"]["y"].as_array().unwrap();
        let z = map["chart"]["heatmap"]["z"].as_array().unwrap();
        assert!(hz.len() > 32, "{map}");
        assert_eq!(z.len(), hz.len());
        assert_eq!(z[0].as_array().unwrap().len(), map["chart"]["heatmap"]["x"].as_array().unwrap().len());
        let mut loud_hz = 0.0;
        let mut loud_db = -80.0;
        // Below 20 Hz is the motion the test adds: drawn, but not the 0 dB reference.
        assert!(hz[0].as_f64().unwrap() < 20.0, "{map}");
        for (row, freq) in z.iter().zip(hz.iter()).filter(|(_, freq)| freq.as_f64().unwrap() >= 20.0) {
            let db = row[0].as_f64().unwrap();
            if db > loud_db {
                loud_db = db;
                loud_hz = freq.as_f64().unwrap();
            }
        }
        assert!((loud_hz - 80.0).abs() < 8.0, "{map}");
        let top = map["chart"]["heatmap"]["z_max"].as_f64().unwrap();
        assert!(loud_db <= top && loud_db > top - 5.0, "{map}");
        assert_eq!(map["chart"]["heatmap"]["z_min"].as_f64().unwrap(), top - 50.0);
        let missing = picture_bytes(&[], "spectrum", &json!({}));
        assert_eq!(missing["ok"], json!(false));
        assert_eq!(gyro_picture("", "spectrum", &json!({}))["ok"], json!(false));
    }

    #[test]
    fn downloaded_copter_batch_peak_is_in_the_motor_band() {
        let out = gyro_batch_fft("002F00354D53501220303932/log-00003.bin");
        let error = out["error"].as_str().unwrap_or("");
        if error.contains("log not found") || error.contains("not in local storage") {
            return;
        }
        assert_eq!(out["ok"], json!(true), "{out}");
        let hz = out["peak_hz"].as_f64().unwrap();
        assert!(hz >= 20.0, "{out}");
        let spec = &out["spectrogram"];
        assert!(spec["columns"].as_array().unwrap().len() > 1, "{spec}");
        assert!(spec["hz_min"].as_f64().unwrap() <= spec["hz_max"].as_f64().unwrap());
    }

    fn rate_row(buf: &mut Vec<u8>, us: u64, desired: f32, actual: f32) {
        buf.extend_from_slice(&[HEAD1, HEAD2, 129]);
        buf.extend_from_slice(&us.to_le_bytes());
        buf.extend_from_slice(&desired.to_le_bytes());
        buf.extend_from_slice(&actual.to_le_bytes());
    }

    #[test]
    fn track_pairs_the_overshoot_with_that_command() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 129, "RATE", "Qff", "TimeUS,RDes,R");
        rate_row(&mut buf, 0, 0.0, 0.0);
        rate_row(&mut buf, 100_000, 50.0, 40.0);
        rate_row(&mut buf, 200_000, 100.0, 90.0);
        rate_row(&mut buf, 300_000, 100.0, 130.0);
        rate_row(&mut buf, 400_000, 20.0, 80.0);
        rate_row(&mut buf, 500_000, 0.0, 25.0);
        rate_row(&mut buf, 1_200_000, 0.0, -180.0);
        let out = track_bytes(&buf, "RATE", "R", None, None);
        assert_eq!(out["actual"], json!("R"));
        assert_eq!(out["desired"], json!("RDes"));
        assert_eq!(out["past_command"]["desired"], json!(100.0));
        assert_eq!(out["past_command"]["actual"], json!(130.0));
        assert_eq!(out["past_command"]["excess"], json!(30.0));
        assert_eq!(out["release"]["actual"], json!(80.0));
        assert_eq!(out["release"]["command"], json!(100.0));
        assert_eq!(out["release"]["delay_s"], json!(0.1));
        assert_eq!(out["release"]["crossings"], json!(0));
        assert_eq!(out["max_abs_desired"], json!(100.0));
        assert_eq!(out["active_count"], json!(3));
        assert_eq!(out["active_error_rms"], json!(19.1));
        assert_eq!(out["peak_error"]["actual"], json!(-180.0));
        assert_eq!(out["peak_error"]["opposite_sign"], json!(false));
        assert!(out["step"].is_null(), "{out}");
        let missing = track_bytes(&buf, "RATE", "Y", None, None);
        assert!(missing["error"].as_str().unwrap().contains("actual column"));
    }

    #[test]
    fn a_small_tracking_error_is_not_rounded_to_zero() {
        assert_eq!(super::round_meas(0.034), 0.034);
        assert_eq!(super::round_meas(19.14), 19.1);
    }

    fn sample_row(buf: &mut Vec<u8>, type_id: u8, us: u64, values: &[f32]) {
        buf.extend_from_slice(&[HEAD1, HEAD2, type_id]);
        buf.extend_from_slice(&us.to_le_bytes());
        for value in values {
            buf.extend_from_slice(&value.to_le_bytes());
        }
    }

    #[test]
    fn track_pairs_angle_height_and_the_shaped_target() {
        let mut attitude = Vec::new();
        push_fmt(&mut attitude, 129, "ATT", "Qff", "TimeUS,DesRoll,Roll");
        sample_row(&mut attitude, 129, 0, &[10.0, 8.0]);
        let roll = track_bytes(&attitude, "ATT", "Roll", None, None);
        assert_eq!(roll["actual"], json!("Roll"));
        assert_eq!(roll["desired"], json!("DesRoll"));
        let from_desired = track_bytes(&attitude, "ATT", "DesRoll", None, None);
        assert_eq!(from_desired["actual"], json!("Roll"));

        let mut down = Vec::new();
        push_fmt(&mut down, 130, "PSCD", "Qffffff", "TimeUS,DPD,TPD,PD,DVD,TVD,VD");
        sample_row(&mut down, 130, 0, &[-1.0, -1.2, -1.1, 0.2, 0.3, 0.25]);
        let position = track_bytes(&down, "PSCD", "PD", None, None);
        assert_eq!(position["desired"], json!("TPD"));
        let velocity = track_bytes(&down, "PSCD", "VD", None, None);
        assert_eq!(velocity["desired"], json!("TVD"));

        let mut climb = Vec::new();
        push_fmt(&mut climb, 131, "CTUN", "Qfff", "TimeUS,DAlt,Alt,TAlt");
        sample_row(&mut climb, 131, 0, &[20.0, 19.0, 18.0]);
        let altitude = track_bytes(&climb, "CTUN", "Alt", None, None);
        assert_eq!(altitude["desired"], json!("DAlt"));

        let mut tecs = Vec::new();
        push_fmt(&mut tecs, 132, "TECS", "Qff", "TimeUS,h,hdem");
        sample_row(&mut tecs, 132, 0, &[100.0, 110.0]);
        let height = track_bytes(&tecs, "TECS", "h", None, None);
        assert_eq!(height["actual"], json!("h"));
        assert_eq!(height["desired"], json!("hdem"));

        let mut pid = Vec::new();
        push_fmt(&mut pid, 133, "PIDR", "Qffff", "TimeUS,Tar,Act,FF,DFF");
        sample_row(&mut pid, 133, 0, &[1.0, 1.1, 0.2, 0.3]);
        let feedforward = track_bytes(&pid, "PIDR", "FF", None, None);
        assert!(feedforward["error"].as_str().unwrap().contains("actual column"));
        let act = track_bytes(&pid, "PIDR", "Act", None, None);
        assert_eq!(act["desired"], json!("Tar"));
    }

    #[test]
    fn track_uses_the_short_arc_on_a_heading() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 129, "ANG", "Qff", "TimeUS,DesYaw,Yaw");
        sample_row(&mut buf, 129, 0, &[10.0, 11.0]);
        sample_row(&mut buf, 129, 100_000, &[0.1, 359.5]);
        sample_row(&mut buf, 129, 200_000, &[10.0, 12.0]);
        let out = track_bytes(&buf, "ANG", "Yaw", None, None);
        assert_eq!(out["circular"], json!(true));
        assert_eq!(out["wrapped_samples"], json!(1));
        assert!(out["error_rms"].as_f64().unwrap() < 2.0, "{out}");
        assert!(out["peak_error"]["error"].as_f64().unwrap().abs() <= 2.0, "{out}");

        let mut rate = Vec::new();
        push_fmt(&mut rate, 130, "RATE", "Qfff", "TimeUS,RDes,R,P");
        sample_row(&mut rate, 130, 0, &[0.0, 0.0, 40.0]);
        sample_row(&mut rate, 130, 100_000, &[100.0, 140.0, 55.0]);
        let roll = track_bytes(&rate, "RATE", "R", None, None);
        assert_eq!(roll["circular"], json!(false));
        assert!(roll["past_command"]["excess"].as_f64().unwrap() > 0.0, "{roll}");
        assert!(roll["past_command"].get("p").is_none(), "{roll}");
    }

    #[test]
    fn step_on_the_search_edge_is_not_a_frequency() {
        let mut rows = Vec::new();
        for i in 0..20 {
            let desired = if i == 0 { 0.0 } else { 20.0 };
            rows.push(TrackRow { us: i * 20_000, desired, actual: desired, p: None, i: None, d: None, ff: None, dmod: None, output: None });
        }
        assert!(step_of(&rows).is_null());
    }

    #[test]
    fn track_keeps_the_d_term_on_that_row() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 130, "PIDR", "Qffffff", "TimeUS,Tar,Act,P,I,D,FF");
        for (us, tar, act, p, i, d, ff) in [
            (0u64, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.5f32, 0.0f32),
            (100_000, 50.0, 40.0, 0.01, 0.01, 0.01, 0.01),
            (200_000, 100.0, 90.0, 0.01, 0.01, 0.01, 0.01),
            (300_000, 100.0, 140.0, 0.11, 0.02, 0.22, 0.03),
            (400_000, 10.0, 30.0, 0.04, 0.04, 0.05, 0.04),
        ] {
            buf.extend_from_slice(&[HEAD1, HEAD2, 130]);
            buf.extend_from_slice(&us.to_le_bytes());
            buf.extend_from_slice(&tar.to_le_bytes());
            buf.extend_from_slice(&act.to_le_bytes());
            buf.extend_from_slice(&p.to_le_bytes());
            buf.extend_from_slice(&i.to_le_bytes());
            buf.extend_from_slice(&d.to_le_bytes());
            buf.extend_from_slice(&ff.to_le_bytes());
        }
        let out = track_bytes(&buf, "PIDR", "Act", None, None);
        assert_eq!(out["past_command"]["excess"], json!(40.0));
        assert_eq!(out["past_command"]["p"], json!(0.11));
        assert_eq!(out["past_command"]["i"], json!(0.02));
        assert_eq!(out["past_command"]["d"], json!(0.22));
        assert_eq!(out["past_command"]["ff"], json!(0.03));
        assert_eq!(out["release"]["d"], json!(0.05));
    }

    #[test]
    fn track_marks_a_sign_change_and_counts_crossings() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 129, "RATE", "Qff", "TimeUS,RDes,R");
        for (us, desired, actual) in [
            (0u64, 0.0f32, 0.0f32),
            (100_000, 40.0, 5.0),
            (200_000, 100.0, -40.0),
            (300_000, 10.0, 30.0),
            (400_000, 0.0, -5.0),
            (500_000, 0.0, 8.0),
            (900_000, 0.0, 1.0),
        ] {
            rate_row(&mut buf, us, desired, actual);
        }
        let out = track_bytes(&buf, "RATE", "R", None, None);
        assert_eq!(out["peak_error"]["desired"], json!(100.0));
        assert_eq!(out["peak_error"]["actual"], json!(-40.0));
        assert_eq!(out["peak_error"]["opposite_sign"], json!(true));
        assert_eq!(out["past_command"], json!(null));
        assert_eq!(out["sides"]["active_count"], json!(2), "{out}");
        assert_eq!(out["sides"]["beyond"]["count"], json!(0), "{out}");
        assert_eq!(out["sides"]["short"]["mean"], json!(35.0), "{out}");
        assert_eq!(out["sides"]["short"]["share"], json!(0.5), "{out}");
        assert_eq!(out["release"]["actual"], json!(30.0));
        assert_eq!(out["release"]["delay_s"], json!(null));
        assert_eq!(out["release"]["crossings"], json!(2));
        assert_eq!(out["active_count"], json!(2));
        assert_eq!(out["active_error_rms"], json!(102.0));
    }

    #[test]
    fn track_fits_a_held_command() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 129, "RATE", "Qff", "TimeUS,RDes,R");
        let wn = 2.0 * std::f64::consts::PI * 5.0;
        for i in 0..40 {
            let t = i as f64 * 0.02;
            let desired = if i == 0 { 0.0 } else { 100.0 };
            let actual = if i == 0 { 0.0 } else { 100.0 * step_model(t - 0.02, 0.5, wn) };
            rate_row(&mut buf, (t * 1_000_000.0) as u64, desired, actual as f32);
        }
        let out = track_bytes(&buf, "RATE", "R", None, None);
        let zeta = out["step"]["zeta"].as_f64().expect(&out.to_string());
        let wn_hz = out["step"]["wn_hz"].as_f64().unwrap();
        assert!((zeta - 0.5).abs() < 0.2, "{out}");
        assert!((wn_hz - 5.0).abs() < 1.5, "{out}");
        assert_eq!(out["hold_count"], json!(1), "{out}");
        assert_eq!(out["holds"][0]["command"], json!(100.0), "{out}");
        assert_eq!(out["holds"][0]["from"], json!(0.0), "{out}");
        assert_eq!(out["holds"][0]["reached"], json!(true), "{out}");
        assert!(out["holds"][0]["zeta"].is_null(), "{out}");
    }

    #[test]
    fn holds_keep_each_settled_change() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 129, "RATE", "Qff", "TimeUS,RDes,R");
        let mut us = 0u64;
        for _ in 0..8 {
            rate_row(&mut buf, us, 0.0, 0.0);
            us += 50_000;
        }
        for _ in 0..8 {
            rate_row(&mut buf, us, 10.0, 16.0);
            us += 50_000;
        }
        let over = track_bytes(&buf, "RATE", "R", None, None);
        assert_eq!(over["hold_count"], json!(1), "{over}");
        assert_eq!(over["holds"][0]["from"], json!(0.0), "{over}");
        assert_eq!(over["holds"][0]["command"], json!(10.0), "{over}");
        assert_eq!(over["holds"][0]["overshoot"], json!(6.0), "{over}");
        assert_eq!(over["sides"]["beyond"]["mean"], json!(6.0), "{over}");
        assert_eq!(over["sides"]["beyond"]["share"], json!(1.0), "{over}");
        assert!(over["sides"]["short"]["mean"].is_null(), "{over}");
        assert_eq!(over["holds"][0]["reached"], json!(true), "{over}");
        assert!((over["holds"][0]["hold_s"].as_f64().unwrap() - 0.35).abs() < 0.001, "{over}");

        let mut flat = Vec::new();
        push_fmt(&mut flat, 129, "RATE", "Qff", "TimeUS,RDes,R");
        us = 0;
        for i in 0..30 {
            rate_row(&mut flat, us, i as f32, i as f32);
            us += 50_000;
        }
        let ramp = track_bytes(&flat, "RATE", "R", None, None);
        assert_eq!(ramp["hold_count"], json!(0), "{ramp}");
        assert!(ramp["holds"].as_array().unwrap().is_empty(), "{ramp}");
        assert_eq!(ramp["sides"]["beyond"]["count"], json!(0), "{ramp}");
        assert_eq!(ramp["sides"]["short"]["count"], json!(0), "{ramp}");
        assert!(ramp["sides"]["beyond"]["mean"].is_null(), "{ramp}");

        let mut missed = Vec::new();
        push_fmt(&mut missed, 129, "RATE", "Qff", "TimeUS,RDes,R");
        us = 0;
        for _ in 0..8 {
            rate_row(&mut missed, us, 0.0, 0.0);
            us += 50_000;
        }
        for _ in 0..8 {
            rate_row(&mut missed, us, 10.0, 1.0);
            us += 50_000;
        }
        let short = track_bytes(&missed, "RATE", "R", None, None);
        assert_eq!(short["holds"][0]["reached"], json!(false), "{short}");
        assert_eq!(short["holds"][0]["overshoot"], json!(0.0), "{short}");

        let mut many = Vec::new();
        push_fmt(&mut many, 129, "RATE", "Qff", "TimeUS,RDes,R");
        us = 0;
        let mut level = 0.0f32;
        rate_row(&mut many, us, level, level);
        us += 50_000;
        for _ in 0..7 {
            rate_row(&mut many, us, level, level);
            us += 50_000;
        }
        for (k, samples) in (4usize..=15).enumerate() {
            level = ((k + 1) * 10) as f32;
            for _ in 0..samples {
                rate_row(&mut many, us, level, level);
                us += 50_000;
            }
        }
        for step in [200.0f32, 280.0, 360.0, 440.0, 520.0] {
            rate_row(&mut many, us, step, step);
            us += 50_000;
        }
        for _ in 0..20 {
            rate_row(&mut many, us, 600.0, 600.0);
            us += 50_000;
        }
        let listed = track_bytes(&many, "RATE", "R", None, None);
        assert_eq!(listed["hold_count"], json!(13), "{listed}");
        let commands: Vec<f64> = listed["holds"].as_array().unwrap().iter().map(|entry| entry["command"].as_f64().unwrap()).collect();
        assert_eq!(commands.len(), 10, "{listed}");
        assert!(!commands.contains(&20.0), "{listed}");
        assert!(!commands.contains(&30.0), "{listed}");
        assert!(!commands.contains(&600.0), "{listed}");
        assert_eq!(commands, vec![10.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0], "{listed}");
    }

    #[test]
    fn sides_split_past_and_short_without_the_opposite_sign() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 129, "RATE", "Qff", "TimeUS,RDes,R");
        for (us, desired, actual) in [
            (0u64, 10.0f32, 80.0),
            (100_000, 100.0, 140.0),
            (200_000, 100.0, 160.0),
            (300_000, 80.0, 50.0),
            (400_000, 40.0, -10.0),
            (500_000, 100.0, 100.0),
            (600_000, 0.0, 0.0),
        ] {
            rate_row(&mut buf, us, desired, actual);
        }
        let out = track_bytes(&buf, "RATE", "R", None, None);
        assert_eq!(out["actual"], json!("R"), "{out}");
        assert_eq!(out["desired"], json!("RDes"), "{out}");
        assert_eq!(out["sides"]["active_count"], json!(5), "{out}");
        assert_eq!(out["sides"]["beyond"]["count"], json!(2), "{out}");
        assert_eq!(out["sides"]["beyond"]["share"], json!(0.4), "{out}");
        assert_eq!(out["sides"]["beyond"]["mean"], json!(50.0), "{out}");
        assert_eq!(out["sides"]["beyond"]["p95"], json!(60.0), "{out}");
        assert_eq!(out["sides"]["short"]["count"], json!(1), "{out}");
        assert_eq!(out["sides"]["short"]["share"], json!(0.2), "{out}");
        assert_eq!(out["sides"]["short"]["mean"], json!(30.0), "{out}");
        assert_eq!(out["sides"]["short"]["p95"], json!(30.0), "{out}");
    }

    #[test]
    fn a_heading_is_not_split_into_sides() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 129, "ANG", "Qff", "TimeUS,DesYaw,Yaw");
        for (us, desired, actual) in [(0u64, 20.0f32, 10.0), (100_000, 40.0, 80.0), (200_000, 10.0, 20.0)] {
            sample_row(&mut buf, 129, us, &[desired, actual]);
        }
        let yaw = track_bytes(&buf, "ANG", "Yaw", None, None);
        assert_eq!(yaw["circular"], json!(true), "{yaw}");
        assert!(yaw["sides"].is_null(), "{yaw}");

        let mut rate = Vec::new();
        push_fmt(&mut rate, 129, "RATE", "Qff", "TimeUS,RDes,R");
        for (us, desired, actual) in [(0u64, 20.0f32, 10.0), (100_000, 40.0, 80.0), (200_000, 10.0, 20.0)] {
            rate_row(&mut rate, us, desired, actual);
        }
        let linear = track_bytes(&rate, "RATE", "R", None, None);
        assert_eq!(linear["sides"]["beyond"]["mean"], json!(25.0), "{linear}");
        assert_eq!(linear["sides"]["beyond"]["p95"], json!(40.0), "{linear}");
        assert_eq!(linear["sides"]["beyond"]["share"], json!(0.667), "{linear}");
        assert_eq!(linear["sides"]["short"]["mean"], json!(10.0), "{linear}");
        assert_eq!(linear["sides"]["short"]["share"], json!(0.333), "{linear}");
    }

    #[test]
    fn frf_measures_a_delay() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 129, "RATE", "Qff", "TimeUS,RDes,R");
        // A command that holds for a pseudo-random time, and the same series five samples later.
        let mut seed = 4321u32;
        let mut next = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f64 / (1u32 << 24) as f64
        };
        let (mut level, mut hold) = (0.0f64, 0usize);
        let mut past = std::collections::VecDeque::from(vec![0.0f64; 5]);
        for i in 0..3000u64 {
            if hold == 0 {
                level = (next() - 0.5) * 60.0;
                hold = 3 + (next() * 20.0) as usize;
            }
            hold -= 1;
            past.push_back(level);
            let actual = past.pop_front().unwrap();
            rate_row(&mut buf, i * 10_000, level as f32, actual as f32);
        }
        let out = frf_bytes(&buf, "RATE", "R", None, None);
        assert_eq!(out["ok"], json!(true), "{out}");
        assert!(out["not_measured"].is_null(), "{out}");
        let delay = out["group_delay_s"].as_f64().expect(&out.to_string());
        assert!((delay - 0.05).abs() < 0.03, "{out}");
        let bins = out["bins"].as_array().unwrap();
        let near = bins.iter().min_by(|a, b| {
            let da = (a["hz"].as_f64().unwrap() - 5.0).abs();
            let db = (b["hz"].as_f64().unwrap() - 5.0).abs();
            da.partial_cmp(&db).unwrap()
        }).unwrap();
        assert!(near["coherence"].as_f64().unwrap() > 0.4, "{near}");
        assert!(near["gain_db"].as_f64().unwrap().abs() < 8.0, "{near}");
    }

    #[test]
    fn a_line_that_moves_is_not_a_line_that_stays() {
        let points: Vec<RidgePoint> = (0..6).map(|i| RidgePoint {
            time_us: i * 1_000_000,
            hz: 80.0 + i as f64 * 12.0,
            amp: 10.0,
            more: [(50.0, 8.0), (0.0, 0.0)],
        }).collect();
        let refs: Vec<&RidgePoint> = points.iter().collect();
        let throttle: Vec<(u64, f64)> = (0..6).map(|i| (i * 1_000_000, 0.2 + i as f64 * 0.1)).collect();
        let lines = spectrum_lines(&refs, &throttle);
        let moving = lines.iter().find(|line| line["follows_throttle"] == json!(true)).expect(&format!("{lines:?}"));
        let fixed = lines.iter().find(|line| line["follows_throttle"] == json!(false)).expect(&format!("{lines:?}"));
        assert!(moving["hz_high"].as_f64().unwrap() > moving["hz_low"].as_f64().unwrap() + 20.0, "{moving}");
        assert!((fixed["hz"].as_f64().unwrap() - 50.0).abs() < 5.0, "{fixed}");
    }

    #[test]
    fn downloaded_rate_track_pairs_each_sample() {
        let roll = compute("002F00354D53501220303932/log-00006.bin", "RATE", "R", "track", Some(20_000_000), Some(100_000_000));
        let error = roll["error"].as_str().unwrap_or("");
        if error.contains("log not found") || error.contains("not in local storage") || error.contains("No such file") {
            return;
        }
        assert_eq!(roll["ok"], json!(true), "{roll}");
        assert!(roll["past_command"]["desired"].is_number(), "{roll}");
        assert!(roll["past_command"]["actual"].is_number(), "{roll}");
        assert!(roll["release"]["actual"].is_number() || roll["release"].is_null(), "{roll}");
    }

    #[test]
    fn tracking_keeps_the_highest_frequency_off_the_highest_throttle() {
        let columns = vec![
            json!({"db": 0.0, "hz": 100.0, "t_s": 1.0, "throttle": 0.2}),
            json!({"db": -1.0, "hz": 200.0, "t_s": 2.0, "throttle": 0.4}),
            json!({"db": -2.0, "hz": 150.0, "t_s": 3.0, "throttle": 0.6}),
            json!({"db": -40.0, "hz": 20.0, "t_s": 0.0, "throttle": 0.0}),
        ];
        let tracking = tracking_of(&columns);
        assert_eq!(tracking["loud_columns"], json!(3));
        assert_eq!(tracking["lowest_hz"]["hz"], json!(100.0));
        assert_eq!(tracking["lowest_hz"]["throttle"], json!(0.2));
        assert_eq!(tracking["highest_hz"]["throttle"], json!(0.4));
        assert_eq!(tracking["at_max_throttle"]["hz"], json!(150.0));
        assert_eq!(tracking["at_max_throttle"]["throttle"], json!(0.6));
        assert!(tracking["throttle_hz_r"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn missing_message_names_the_argument() {
        let out = compute("unused", "", "GyrX", "fft", None, None);
        let error = out["error"].as_str().unwrap();
        assert!(error.contains("message is required"));
        assert!(!error.contains("GyrX"));
    }

    #[test]
    fn gps_week_is_a_utc_date() {
        assert_eq!(unix_rfc3339(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(gps_rfc3339(2288, 252_818_000).as_deref(), Some("2023-11-14T22:13:20Z"));
        assert_eq!(onboard_id("002F/log-00032.bin"), Some(32));
        assert_eq!(onboard_id("002F/0123456789abcdef-6.bin"), Some(6));
        assert_eq!(onboard_id("002F/0123456789abcdef_20231114221320_6.bin"), Some(6));
        assert!(is_stored_copy("002F/094d906674953735_20260929185818_3.bin", 8_449_246, 3, 8_449_246));
        assert!(!is_stored_copy("002F/094d906674953735_20260929185818_3.bin", 8_449_246, 3, 5_000_000));
        assert!(!is_stored_copy("002F/094d906674953735_20260929185818_3.bin", 0, 3, 0));
        let named = downloaded_log_name(6, b"same", 1_700_000_000);
        assert!(named.ends_with("_20231114221320_6.bin"), "{named}");
        assert_eq!(named, downloaded_log_name(6, b"same", 1_700_000_000));
        assert_ne!(named, downloaded_log_name(6, b"other", 1_700_000_000));
        let foreign = import_log(b"not a log", "flight.bin").unwrap_err();
        assert_eq!(foreign, "This file is not a DataFlash log.");
        let mut named_log = Vec::new();
        push_fmt(&mut named_log, 160, "MSG", "QZ", "TimeUS,Message");
        push_qz(&mut named_log, 160, 1, "JHEM_JHEF405 002F0035 4D535012 20303932");
        assert_eq!(vehicle_dir(&named_log), "002F00354D53501220303932");
        let mut unnamed = Vec::new();
        push_fmt(&mut unnamed, 160, "MSG", "QZ", "TimeUS,Message");
        push_qz(&mut unnamed, 160, 1, "ArduCopter V4.7.1");
        assert_eq!(vehicle_dir(&unnamed), "imported");
        let mut late = Vec::new();
        push_fmt(&mut late, 160, "MSG", "QZ", "TimeUS,Message");
        for n in 0..2000 {
            push_qz(&mut late, 160, n, "PreArm: waiting");
        }
        push_qz(&mut late, 160, 2001, "JHEM_JHEF405 002F0035 4D535012 20303932");
        assert_eq!(vehicle_dir(&late), "002F00354D53501220303932");
    }

    #[test]
    fn imported_logs_keep_the_file_name() {
        assert_eq!(imported_stem("00000045.BIN").as_deref(), Some("00000045"));
        assert_eq!(imported_stem(r"C:\logs\політ 1.bin").as_deref(), Some("політ_1"));
        assert_eq!(imported_stem("a/b:c?.bin").as_deref(), Some("b_c"));
        assert_eq!(imported_stem("flight.tlog.bin").as_deref(), Some("flight.tlog"));
        assert_eq!(imported_stem("").as_deref(), None);
        assert_eq!(imported_stem("....bin").as_deref(), None);
        let parts = stored_name_parts("imported/0123456789abcdef_00000045.bin");
        assert_eq!(parts["sha256_16"], "0123456789abcdef");
        assert_eq!(parts["original_name"], "00000045.bin");
        let auto = stored_name_parts("002F/0123456789abcdef_20231114221320_6.bin");
        assert_eq!(auto["flight_stamp"], "20231114221320");
        assert!(auto.get("original_name").is_none());
    }

    #[test]
    fn first_gps_time_stops_at_the_fix() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 130, "GPS", "QBIH", "TimeUS,Status,GMS,GWk");
        buf.extend_from_slice(&[HEAD1, HEAD2, 130]);
        buf.extend_from_slice(&1u64.to_le_bytes());
        buf.push(3);
        buf.extend_from_slice(&252_818_000u32.to_le_bytes());
        buf.extend_from_slice(&2288u16.to_le_bytes());
        assert_eq!(first_flight_utc(&buf).as_deref(), Some("2023-11-14T22:13:20Z"));
        let named = downloaded_log_name(2, &buf, 1_600_000_000);
        assert!(named.ends_with("_20231114221320_2.bin"), "{named}");
    }

    fn plain_row(us: u64, desired: f64, actual: f64) -> TrackRow {
        TrackRow { us, desired, actual, p: None, i: None, d: None, ff: None, dmod: None, output: None }
    }

    #[test]
    fn phases_keep_repeated_modes_as_separate_stretches() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 129, "MODE", "Qf", "TimeUS,ModeNum");
        push_fmt(&mut buf, 130, "ARM", "Qf", "TimeUS,ArmState");
        push_fmt(&mut buf, 131, "RATE", "Qf", "TimeUS,R");
        sample_row(&mut buf, 130, 10_000_000, &[1.0]);
        sample_row(&mut buf, 129, 10_000_000, &[2.0]);
        sample_row(&mut buf, 129, 20_000_000, &[22.0]);
        sample_row(&mut buf, 129, 25_000_000, &[2.0]);
        sample_row(&mut buf, 130, 30_000_000, &[0.0]);
        sample_row(&mut buf, 131, 31_000_000, &[0.0]);
        let (modes, armed) = flight_intervals(&buf);
        let modes = modes.as_array().unwrap();
        let armed = armed.as_array().unwrap();
        assert_eq!(modes.len(), 3);
        assert_eq!(modes[0]["state"], json!(2));
        assert_eq!(modes[0]["end_us"], json!(20_000_000));
        assert_eq!(modes[1]["state"], json!(22));
        assert_eq!(modes[2]["start_us"], json!(25_000_000));
        assert_eq!(modes[2]["end_us"], json!(31_000_000));
        assert_eq!(armed[0]["state"], json!(1));
        assert_eq!(armed[0]["end_us"], json!(30_000_000));
        assert_eq!(armed[1]["state"], json!(0));
        assert_eq!(armed[1]["end_us"], json!(31_000_000));
    }

    #[test]
    fn delay_zero_means_actual_was_already_there() {
        let already = vec![plain_row(0, 0.0, 80.0), plain_row(100_000, 100.0, 90.0)];
        let (delay, at_half) = step_delay_s(&already, &already[1]);
        assert!(delay.is_none());
        assert!(at_half);
        let rose = vec![plain_row(0, 0.0, 0.0), plain_row(100_000, 100.0, 10.0), plain_row(200_000, 100.0, 60.0)];
        let (delay, at_half) = step_delay_s(&rose, &rose[1]);
        assert_eq!(delay, Some(0.1));
        assert!(!at_half);
    }

    #[test]
    fn release_span_shows_one_move_beside_the_sign_count() {
        let rows = vec![
            plain_row(0, 0.0, 0.0),
            plain_row(100_000, 100.0, 50.0),
            plain_row(200_000, 0.5, 0.9),
            plain_row(300_000, 0.3, 0.6),
            plain_row(400_000, 0.1, 0.4),
            plain_row(500_000, -0.1, 0.0),
            plain_row(600_000, -0.4, -0.8),
        ];
        let motion = release_motion(&rows, &rows[1], false);
        assert_eq!(motion.actual_from, 0.9);
        assert_eq!(motion.actual_to, -0.8);
        assert_eq!(motion.crossings, 1);
        assert!((motion.error_span - 0.8).abs() < 0.05, "{}", motion.error_span);
    }

    #[test]
    fn track_reports_the_body_of_the_error_and_the_pid_terms() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 130, "PIDR", "Qfffffff", "TimeUS,Tar,Act,P,I,D,FF,Dmod");
        for (i, (tar, act)) in [(0.0f32, 0.0f32), (1.0, 2.0), (2.0, 4.0), (3.0, 6.0), (4.0, 8.0)].iter().enumerate() {
            buf.extend_from_slice(&[HEAD1, HEAD2, 130]);
            buf.extend_from_slice(&(i as u64 * 100_000).to_le_bytes());
            for value in [*tar, *act, 0.1, 0.01, 0.1, 0.02, 1.0] {
                buf.extend_from_slice(&value.to_le_bytes());
            }
        }
        let out = track_bytes(&buf, "PIDR", "Act", None, None);
        assert_eq!(out["error_p95"], json!(4.0), "{out}");
        assert_eq!(out["corr"], json!(1.0), "{out}");
        assert_eq!(out["spread"], json!(2.0), "{out}");
        assert_eq!(out["sample_hz"], json!(10.0), "{out}");
        assert_eq!(out["p_rms"], json!(0.1), "{out}");
        assert_eq!(out["p_share"], json!(0.488), "{out}");
        assert_eq!(out["i_share"], json!(0.005), "{out}");
        assert_eq!(out["d_share"], json!(0.488), "{out}");
        assert_eq!(out["ff_share"], json!(0.020), "{out}");
        assert!(out["error_peak_hz"].is_null(), "{out}");
        assert_eq!(out["dmod_min"], json!(1.0), "{out}");

        let mut yaw = Vec::new();
        push_fmt(&mut yaw, 131, "ANG", "Qff", "TimeUS,DesYaw,Yaw");
        sample_row(&mut yaw, 131, 0, &[10.0, 350.0]);
        sample_row(&mut yaw, 131, 100_000, &[20.0, 30.0]);
        let heading = track_bytes(&yaw, "ANG", "Yaw", None, None);
        assert!(heading["corr"].is_null(), "{heading}");
        assert!(heading["spread"].is_null(), "{heading}");
    }

    #[test]
    fn track_names_a_tone_in_the_error() {
        let rows: Vec<TrackRow> = (0..64).map(|i| {
            let t = i as f64 * 0.01;
            plain_row(i * 10_000, 0.0, (2.0 * std::f64::consts::PI * 5.0 * t).sin())
        }).collect();
        let out = track_rows(&rows, false);
        let hz = out["error_peak_hz"].as_f64().expect(&out.to_string());
        let share = out["error_peak_share"].as_f64().expect(&out.to_string());
        assert!((hz - 5.0).abs() < 1.0, "{out}");
        assert!(share > 0.15, "{out}");
        assert!(out["p_share"].is_null(), "{out}");
    }

    #[test]
    fn parm_rows_are_the_value_recorded_in_the_file() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 140, "PARM", "QNf", "TimeUS,Name,Value");
        let row = |buf: &mut Vec<u8>, us: u64, name: &str, value: f32| {
            buf.extend_from_slice(&[HEAD1, HEAD2, 140]);
            buf.extend_from_slice(&us.to_le_bytes());
            let mut bytes = [0u8; 16];
            bytes[..name.len()].copy_from_slice(name.as_bytes());
            buf.extend_from_slice(&bytes);
            buf.extend_from_slice(&value.to_le_bytes());
        };
        row(&mut buf, 0, "ATC_RAT_RLL_P", 0.5);
        row(&mut buf, 1_000_000, "ATC_RAT_RLL_P", 0.25);
        let out = parm_bytes(&buf, &["ATC_RAT_RLL_P".into(), "ATC_RAT_YAW_P".into()]);
        let params = out["params"].as_array().unwrap();
        assert_eq!(params[0]["value"], json!(0.25));
        assert_eq!(params[0]["first"], json!(0.5));
        assert_eq!(params[0]["changed"], json!(true));
        assert_eq!(params[1]["known"], json!(false));
    }

    #[test]
    fn track_lag_is_the_shift_of_the_interval_and_output_is_its_own_peak() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 130, "RATE", "Qfff", "TimeUS,RDes,R,ROut");
        for i in 0..20 {
            let desired = i as f32;
            let actual = if i < 2 { 0.0 } else { (i - 2) as f32 };
            buf.extend_from_slice(&[HEAD1, HEAD2, 130]);
            buf.extend_from_slice(&(i as u64 * 50_000).to_le_bytes());
            for value in [desired, actual, 1.0f32] {
                buf.extend_from_slice(&value.to_le_bytes());
            }
        }
        let out = track_bytes(&buf, "RATE", "R", None, None);
        assert_eq!(out["lag_s"], json!(0.1), "{out}");
        assert_eq!(out["output_abs_max"], json!(1.0), "{out}");
        assert_eq!(out["output_at_peak"], json!(1.0), "{out}");
    }

    #[test]
    fn track_states_the_unit_the_log_recorded() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 117, "UNIT", "QbZ", "TimeUS,Id,Label");
        push_fmt(&mut buf, 116, "FMTU", "QBNN", "TimeUS,FmtType,UnitIds,MultIds");
        push_fmt(&mut buf, 130, "RATE", "Qff", "TimeUS,RDes,R");
        push_fmt(&mut buf, 131, "PIDR", "Qff", "TimeUS,Tar,Act");
        let mut unit = |id: i8, label: &str| {
            buf.extend_from_slice(&[HEAD1, HEAD2, 117]);
            buf.extend_from_slice(&0u64.to_le_bytes());
            buf.push(id as u8);
            let mut text = [0u8; 64];
            text[..label.len()].copy_from_slice(label.as_bytes());
            buf.extend_from_slice(&text);
        };
        unit(b's' as i8, "s");
        unit(b'k' as i8, "deg/s");
        let mut fmtu = |type_id: u8, codes: &str| {
            buf.extend_from_slice(&[HEAD1, HEAD2, 116]);
            buf.extend_from_slice(&0u64.to_le_bytes());
            buf.push(type_id);
            let mut ids = [0u8; 16];
            ids[..codes.len()].copy_from_slice(codes.as_bytes());
            buf.extend_from_slice(&ids);
            buf.extend_from_slice(&[0u8; 16]);
        };
        fmtu(130, "skk");
        fmtu(131, "s--");
        for i in 0..2 {
            buf.extend_from_slice(&[HEAD1, HEAD2, 130]);
            buf.extend_from_slice(&(i as u64 * 100_000).to_le_bytes());
            buf.extend_from_slice(&1.0f32.to_le_bytes());
            buf.extend_from_slice(&1.0f32.to_le_bytes());
            buf.extend_from_slice(&[HEAD1, HEAD2, 131]);
            buf.extend_from_slice(&(i as u64 * 100_000).to_le_bytes());
            buf.extend_from_slice(&0.4f32.to_le_bytes());
            buf.extend_from_slice(&0.4f32.to_le_bytes());
        }
        let rate = track_bytes(&buf, "RATE", "R", None, None);
        assert_eq!(rate["actual_unit"], json!("deg/s"), "{rate}");
        assert_eq!(rate["desired_unit"], json!("deg/s"), "{rate}");
        let pid = track_bytes(&buf, "PIDR", "Act", None, None);
        assert!(pid["actual_unit"].is_null(), "{pid}");
        assert!(pid["desired_unit"].is_null(), "{pid}");
        assert!((pid["error_rms"].as_f64().unwrap() - 0.0).abs() < 1e-9);
        let note = rate["note"].as_str().unwrap();
        assert!(note.contains("`spread` = std(`actual`) / std(`desired`).\n"), "{note}");
        assert!(note.contains("It is not a percent.\n"), "{note}");
        assert!(note.contains("The sentence is that the columns line up at this shift.\n"), "{note}");
        assert!(note.contains("It is not evidence that D is high, and it is not a reason to change D.\n"), "{note}");
        assert!(note.contains("A modest `corr` beside a small `error_rms` is a hold.\n"), "{note}");
        assert!(note.contains("not a frequency, not an overshoot, not damping, and not a gain."), "{note}");
        assert!(note.contains("`beyond` = |`actual`| - |`desired`|, where that is positive."), "{note}");
        assert!(note.contains("No samples on a side leaves `mean` and `p95` null."), "{note}");
        assert!(note.contains("It does not say why actual varied more, and it does not name a parameter."), "{note}");
        assert!(!note.contains("percent past the command"), "{note}");
        assert!(!note.contains("varied more than the command"), "{note}");
    }

    fn push_rate(buf: &mut Vec<u8>, type_id: u8, time: u64, actual: f32, desired: f32) {
        buf.extend_from_slice(&[HEAD1, HEAD2, type_id]);
        buf.extend_from_slice(&time.to_le_bytes());
        buf.extend_from_slice(&actual.to_le_bytes());
        buf.extend_from_slice(&desired.to_le_bytes());
    }

    #[test]
    fn a_chart_draws_every_sample() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 130, "RATE", "Qff", "TimeUS,R,RDes");
        for i in 0..900u64 {
            let y = if i == 450 { 50.0 } else { 0.1 };
            push_rate(&mut buf, 130, i * 20_000, y, 0.2);
        }
        let spec = json!({
            "title": "Pitch rate",
            "slug": " pitch ",
            "axes": [
                { "label": "deg/s", "lines": [
                    { "message": "RATE", "field": "R", "name": "actual" },
                    { "message": "RATE", "field": "RDes", "name": "desired" }
                ]},
                { "label": "extra", "lines": [ { "message": "RATE", "field": "Nope" } ]}
            ]
        });
        let out = chart_bytes(&buf, &spec);
        assert_eq!(out["ok"], json!(true), "{out}");
        assert_eq!(out["slug"], json!("pitch"));
        assert_eq!(out["chart"]["slug"], json!("pitch"));
        assert_eq!(out["chart"]["x_label"], json!("s"));
        let actual = &out["chart"]["axes"][0]["lines"][0];
        assert_eq!(actual["name"], json!("actual"));
        let n = actual["y"].as_array().unwrap().len();
        assert_eq!(n, 900, "{n}");
        let peak = actual["y"].as_array().unwrap().iter().filter_map(|v| v.as_f64()).fold(0.0_f64, f64::max);
        assert!((peak - 50.0).abs() < 1e-6, "{peak}");
        assert_eq!(out["chart"]["axes"][0]["lines"][1]["name"], json!("desired"));
        assert!(out["note"].as_str().unwrap().contains("RATE.Nope is not in this log"));
        let missing = chart_bytes(&buf, &json!({ "axes": [] }));
        assert_eq!(missing["ok"], json!(false));
    }

    #[test]
    fn wmm_calculates_expected_earth_field() {
        // Kyiv, Ukraine: lat 50.45, lon 30.52
        let ef = expected_earth_field(50.45, 30.52).expect("expected earth field in range");
        let total_intensity = (ef[0] * ef[0] + ef[1] * ef[1] + ef[2] * ef[2]).sqrt();
        assert!(total_intensity > 450.0 && total_intensity < 550.0, "{total_intensity}");
        assert!(ef[0] > 150.0, "North component: {}", ef[0]);
        assert!(ef[2] > 400.0, "Down component: {}", ef[2]);
    }

    #[test]
    fn quaternion_rotates_vectors_and_interpolates() {
        // Test YAW_90: vector [1, 0, 0] rotated by YAW_90 becomes [0, 1, 0]
        let q_yaw90 = quaternion_from_rotation(2).unwrap();
        let rotated = quaternion_rotate(q_yaw90, [1.0, 0.0, 0.0]);
        assert!((rotated[0]).abs() < 1e-6);
        assert!((rotated[1] - 1.0).abs() < 1e-6);
        assert!((rotated[2]).abs() < 1e-6);

        // Test SLERP at t = 0.5
        let q_none = quaternion_from_rotation(0).unwrap();
        let q_half = quaternion_slerp(q_none, q_yaw90, 0.5);
        let q_yaw45 = quaternion_from_rotation(1).unwrap();
        for i in 0..4 {
            assert!((q_half[i] - q_yaw45[i]).abs() < 1e-4, "idx {i}: {} vs {}", q_half[i], q_yaw45[i]);
        }
    }

    #[test]
    fn linear_solver_and_iron_inversion_solve_accurately() {
        // 3x3 diagonal system
        let a = [
            [2.0, 0.0, 0.0],
            [0.0, 3.0, 0.0],
            [0.0, 0.0, 4.0],
        ];
        let b = [4.0, 9.0, 16.0];
        let sol = solve_linear_system(a, b).unwrap();
        assert!((sol[0] - 2.0).abs() < 1e-9);
        assert!((sol[1] - 3.0).abs() < 1e-9);
        assert!((sol[2] - 4.0).abs() < 1e-9);

        // Symmetric 3x3 inversion
        let dia = [1.0, 1.0, 1.0];
        let odi = [0.0, 0.0, 0.0];
        let inv = invert_symmetric_3x3(dia, odi).unwrap();
        assert!((inv[0][0] - 1.0).abs() < 1e-9);
        assert!((inv[1][1] - 1.0).abs() < 1e-9);
        assert!((inv[2][2] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn magfit_fits_synthetic_log_compass_parameters() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 140, "PARM", "QNf", "TimeUS,Name,Value");
        push_fmt(&mut buf, 150, "AHR2", "Qfff", "TimeUS,Roll,Pitch,Yaw");
        push_fmt(&mut buf, 160, "MAG", "QBffffff", "TimeUS,I,MagX,MagY,MagZ,OfsX,OfsY,OfsZ");

        let push_parm = |buf: &mut Vec<u8>, name: &str, val: f32| {
            buf.extend_from_slice(&[HEAD1, HEAD2, 140]);
            buf.extend_from_slice(&0u64.to_le_bytes());
            let mut name_b = [0u8; 16];
            name_b[..name.len()].copy_from_slice(name.as_bytes());
            buf.extend_from_slice(&name_b);
            buf.extend_from_slice(&val.to_le_bytes());
        };

        push_parm(&mut buf, "AHRS_ORIGIN_LAT", 50.45);
        push_parm(&mut buf, "AHRS_ORIGIN_LON", 30.52);
        push_parm(&mut buf, "COMPASS_OFS_X", 0.0);
        push_parm(&mut buf, "COMPASS_OFS_Y", 0.0);
        push_parm(&mut buf, "COMPASS_OFS_Z", 0.0);
        push_parm(&mut buf, "COMPASS_DIA_X", 1.0);
        push_parm(&mut buf, "COMPASS_DIA_Y", 1.0);
        push_parm(&mut buf, "COMPASS_DIA_Z", 1.0);
        push_parm(&mut buf, "COMPASS_SCALE", 1.0);
        push_parm(&mut buf, "COMPASS_ORIENT", 0.0);

        let ef = expected_earth_field(50.45, 30.52).unwrap();
        let true_ofs = [45.0, -35.0, 25.0];

        for i in 0..72 {
            let t_us = (i + 1) as u64 * 100_000;
            let yaw_deg = (i as f64 * 5.0).to_radians();
            let roll_deg = ((i as f64 * 0.2).sin() * 10.0).to_radians();
            let pitch_deg = ((i as f64 * 0.3).cos() * 8.0).to_radians();

            buf.extend_from_slice(&[HEAD1, HEAD2, 150]);
            buf.extend_from_slice(&t_us.to_le_bytes());
            buf.extend_from_slice(&(roll_deg.to_degrees() as f32).to_le_bytes());
            buf.extend_from_slice(&(pitch_deg.to_degrees() as f32).to_le_bytes());
            buf.extend_from_slice(&(yaw_deg.to_degrees() as f32).to_le_bytes());

            let q = quaternion_from_euler(roll_deg, pitch_deg, yaw_deg);
            let q_inv = [q[0], -q[1], -q[2], -q[3]];
            let e_body = quaternion_rotate(q_inv, ef);

            let measured = [
                e_body[0] + true_ofs[0],
                e_body[1] + true_ofs[1],
                e_body[2] + true_ofs[2],
            ];

            buf.extend_from_slice(&[HEAD1, HEAD2, 160]);
            buf.extend_from_slice(&t_us.to_le_bytes());
            buf.push(0);
            buf.extend_from_slice(&(measured[0] as f32).to_le_bytes());
            buf.extend_from_slice(&(measured[1] as f32).to_le_bytes());
            buf.extend_from_slice(&(measured[2] as f32).to_le_bytes());
            buf.extend_from_slice(&0.0f32.to_le_bytes());
            buf.extend_from_slice(&0.0f32.to_le_bytes());
            buf.extend_from_slice(&0.0f32.to_le_bytes());
        }

        let out = magfit_bytes(&buf, 1, "iron", "none", true, None, None, None, None);
        assert_eq!(out["ok"], json!(true), "{out}");
        assert_eq!(out["compass"], json!(1));
        assert!(out["reduction_pct"].as_f64().unwrap() > 90.0, "reduction was: {}", out["reduction_pct"]);
        assert_eq!(out["orientation"]["matches"], json!(true));

        let p = &out["params"];
        let ofs_x = p["COMPASS_OFS_X"].as_f64().unwrap();
        let ofs_y = p["COMPASS_OFS_Y"].as_f64().unwrap();
        let ofs_z = p["COMPASS_OFS_Z"].as_f64().unwrap();
        assert!((ofs_x + true_ofs[0]).abs() < 1.0, "ofs_x: {ofs_x}");
        assert!((ofs_y + true_ofs[1]).abs() < 1.0, "ofs_y: {ofs_y}");
        assert!((ofs_z + true_ofs[2]).abs() < 1.0, "ofs_z: {ofs_z}");
    }

    #[test]
    fn magfit_fails_without_coords_and_succeeds_with_override() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 140, "PARM", "QNf", "TimeUS,Name,Value");
        push_fmt(&mut buf, 150, "AHR2", "Qfff", "TimeUS,Roll,Pitch,Yaw");
        push_fmt(&mut buf, 160, "MAG", "QBffffff", "TimeUS,I,MagX,MagY,MagZ,OfsX,OfsY,OfsZ");

        // Notice: NO AHRS_ORIGIN_LAT / LON, and NO ORGN / GPS messages in buf.
        let earth = expected_earth_field(50.45, 30.52).unwrap();
        for i in 0..100 {
            let roll = (i as f64 * 3.6).to_radians();
            let pitch = ((i as f64 * 2.5).sin() * 45.0).to_radians();
            let yaw = (i as f64 * 7.2).to_radians();
            let q = quaternion_from_euler(roll, pitch, yaw);
            let q_inv = [q[0], -q[1], -q[2], -q[3]];
            let expected_body = quaternion_rotate(q_inv, earth);

            // AHR2
            buf.extend_from_slice(&[HEAD1, HEAD2, 150]);
            buf.extend_from_slice(&(i as u64 * 20_000).to_le_bytes());
            buf.extend_from_slice(&(roll.to_degrees() as f32).to_le_bytes());
            buf.extend_from_slice(&(pitch.to_degrees() as f32).to_le_bytes());
            buf.extend_from_slice(&(yaw.to_degrees() as f32).to_le_bytes());

            // MAG
            buf.extend_from_slice(&[HEAD1, HEAD2, 160]);
            buf.extend_from_slice(&(i as u64 * 20_000).to_le_bytes());
            buf.push(0); // compass 1
            buf.extend_from_slice(&(expected_body[0] as f32).to_le_bytes());
            buf.extend_from_slice(&(expected_body[1] as f32).to_le_bytes());
            buf.extend_from_slice(&(expected_body[2] as f32).to_le_bytes());
            buf.extend_from_slice(&0.0f32.to_le_bytes());
            buf.extend_from_slice(&0.0f32.to_le_bytes());
            buf.extend_from_slice(&0.0f32.to_le_bytes());
        }

        // Without coords: must fail with informative error
        let fail_res = magfit_bytes(&buf, 1, "iron", "none", true, None, None, None, None);
        assert_eq!(fail_res["ok"], json!(false));
        assert!(fail_res["error"].as_str().unwrap().contains("No GPS or origin coordinates found"));

        // With manual lat and lon: succeeds
        let ok_res = magfit_bytes(&buf, 1, "iron", "none", true, None, None, Some(50.45), Some(30.52));
        assert_eq!(ok_res["ok"], json!(true), "{ok_res}");
        assert_eq!(ok_res["coordinates"]["source"], json!("manual"));
        assert_eq!(ok_res["coordinates"]["lat"], json!(50.45));
        assert_eq!(ok_res["coordinates"]["lon"], json!(30.52));
    }

    /// A log recorded by scripts/sitl-test-logs.py: ArduCopter SITL flying with one known fault.
    fn sitl_log(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/dflog/testdata/sitl").join(format!("{name}.bin"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}. Record it with scripts/sitl-test-logs.py", path.display()))
    }

    #[test]
    #[ignore]
    fn sitl_dump() {
        let which = std::env::var("SITL_DUMP").unwrap_or_default();
        let bytes = sitl_log(&which);
        let res = match which.as_str() {
            "motors" => motor_balance_bytes(&bytes, None, None),
            "notch" => filter_review_bytes(&bytes, None, None, None),
            "magfit" => magfit_bytes(&bytes, 1, "iron", "none", true, None, None, None, None),
            "pid" | "plane_pid" | "sysid" => json!(["roll", "pitch", "yaw"].map(|a| pid_review_bytes(&bytes, Some(a), None, None))),
            "healthy" => json!({
                "audit": audit_bytes(&bytes),
                "filter": filter_review_bytes(&bytes, None, None, None),
                "motors": motor_balance_bytes(&bytes, None, None),
            }),
            _ => audit_bytes(&bytes),
        };
        println!("DUMP {}", serde_json::to_string_pretty(&res).unwrap());
    }

    #[test]
    fn a_log_is_known_by_its_head_whatever_its_length() {
        let log = sitl_log("ekf_lane");
        // The board's file may end earlier or later than its listed size: the head stays the same.
        assert_eq!(head_hash(&log), head_hash(&log[..HEAD_HASH_BYTES]));
        assert_eq!(head_hash(&log), head_hash(&log[..log.len() - 1000]));
        assert_ne!(head_hash(&log), head_hash(&sitl_log("gps_loss")));
        assert_eq!(head_hash(&log).len(), 16);
    }

    #[test]
    fn sitl_motor_balance_finds_the_weak_motor() {
        // Motor 1 gives 8% less thrust (SIM_ENGINE_MUL 0.92 on motor 1): it runs harder than the rest.
        let res = motor_balance_bytes(&sitl_log("motors"), None, None);
        assert_eq!(res["ok"], json!(true), "{res}");
        let motors = res["motors"].as_array().unwrap();
        assert_eq!(motors.len(), 4, "{res}");
        let diff = |i: usize| motors[i]["diff_pct"].as_f64().unwrap();
        assert!(diff(0) > 1.5, "{res}");
        assert!((1..4).all(|i| diff(i) < 0.0), "{res}");
    }

    #[test]
    fn sitl_filter_review_moves_a_misplaced_throttle_notch() {
        // Motor noise near 155 Hz at hover; the throttle notch is set at 60 Hz with REF 0.3.
        let res = filter_review_bytes(&sitl_log("notch"), None, None, None);
        assert_eq!(res["ok"], json!(true), "{res}");
        let peak = res["primary_peak_hz"].as_f64().unwrap();
        assert!((130.0..=185.0).contains(&peak), "{peak}");
        assert!(res["tracked_notch_hz"].as_f64().unwrap() < 90.0, "{res}");
        assert_eq!(res["harmonics"][0]["notched"], json!(false), "{}", res["harmonics"]);
        let proposed = &res["proposed_params"];
        assert!((proposed["INS_HNTCH_FREQ"].as_f64().unwrap() - peak).abs() <= 5.0, "{proposed}");
        assert!((0.25..=0.5).contains(&proposed["INS_HNTCH_REF"].as_f64().unwrap()), "{proposed}");
        // The post-filter batch (INS_LOG_BAT_OPT 4) is read as such, not guessed from its level.
        assert!(res["measured_post_filter"]["attenuation_db"].as_f64().unwrap() > 0.0, "{res}");
    }

    #[test]
    fn sitl_audit_reads_an_ekf_lane_switch() {
        // An accelerometer bias on IMU 1 in flight moves EKF3 to the second lane.
        let res = audit_bytes(&sitl_log("ekf_lane"));
        assert_eq!(res["ok"], json!(true), "{res}");
        let nav = &res["navigation"];
        assert!(nav["ekf_lane_switches"].as_array().unwrap().iter().any(|row| row["new_core"] == json!(1)), "{nav}");
        assert_eq!(nav["armed_min_fix_type"], json!(6), "{nav}");
        assert_eq!(res["health"]["categories"]["ekf"]["status"], json!("WARN"), "{res}");
        let modes: Vec<&str> = res["modes"].as_array().unwrap().iter().filter_map(|row| row["mode"].as_str()).collect();
        assert_eq!(modes, ["GUIDED", "LOITER", "LAND"], "{res}");
    }

    #[test]
    fn sitl_audit_reads_gps_loss_and_the_ekf_failsafe() {
        // GPS switched off in Loiter: EKF failsafe, then Land without position.
        let res = audit_bytes(&sitl_log("gps_loss"));
        assert_eq!(res["ok"], json!(true), "{res}");
        let nav = &res["navigation"];
        assert!(nav["armed_min_fix_type"].as_u64().unwrap() < 3, "{nav}");
        assert_eq!(nav["ekf_failsafes"], json!(1), "{nav}");
        assert_eq!(res["health"]["categories"]["ekf"]["status"], json!("FAIL"), "{res}");
        assert_ne!(res["health"]["categories"]["gps"]["status"], json!("PASS"), "{res}");
        assert!(res["flight"]["last_disarmed_us"].as_u64().unwrap() > 0, "{res}");
        assert_eq!(res["modes"].as_array().unwrap().last().unwrap()["mode"], json!("LAND"), "{res}");
    }

    #[test]
    fn sitl_magfit_finds_the_compass_offsets() {
        // SIM_MAG1_OFS 120, -80, 60 with COMPASS_OFS at 0. Z is the least covered axis in this flight.
        let res = magfit_bytes(&sitl_log("magfit"), 1, "iron", "none", true, None, None, None, None);
        assert_eq!(res["ok"], json!(true), "{res}");
        let p = &res["params"];
        let ofs = |axis: &str| p[format!("COMPASS_OFS_{axis}")].as_f64().unwrap();
        assert!((ofs("X") - 120.0).abs() < 15.0, "{p}");
        assert!((ofs("Y") + 80.0).abs() < 15.0, "{p}");
        assert!((ofs("Z") - 60.0).abs() < 40.0, "{p}");
        assert!(res["fit_error_mgauss"].as_f64().unwrap() < res["orig_error_mgauss"].as_f64().unwrap(), "{res}");
        assert_eq!(res["orientation"]["matches"], json!(true), "{res}");
    }

    #[test]
    fn sitl_pid_review_measures_roll_and_pitch_from_stick_doublets() {
        let bytes = sitl_log("pid");
        for axis in ["roll", "pitch"] {
            let res = pid_review_bytes(&bytes, Some(axis), None, None);
            assert_eq!(res["ok"], json!(true), "{res}");
            assert_eq!(res["controller"], json!("copter"), "{res}");
            let m = &res["metrics"];
            assert!(res.get("step_response_unavailable").map_or(true, Value::is_null), "{axis}: {res}");
            assert!((5.0..=200.0).contains(&m["rise_time_ms"].as_f64().unwrap()), "{axis}: {m}");
            assert!(m["overshoot_pct"].is_number(), "{axis}: {m}");
            assert!((0.0..=90.0).contains(&m["phase_margin_deg"].as_f64().unwrap()), "{axis}: {m}");
            // A 100 Hz log measures up to 50 Hz; the phase may not cross -180° below that, and then
            // there is no gain margin to report.
            assert!(m["gain_margin_db"].as_f64().map_or(true, |gm| gm > 0.0), "{axis}: {m}");
        }
        // Yaw follows its target slowly in these doublets: the result says so instead of a number.
        let yaw = pid_review_bytes(&bytes, Some("yaw"), None, None);
        assert!(yaw["metrics"]["rise_time_ms"].is_null() == yaw["step_response_unavailable"].is_string(), "{yaw}");
    }

    #[test]
    fn sitl_healthy_flight_fires_no_rule() {
        // The control case: a throttle notch on the motor line, failsafes set, nothing wrong.
        let bytes = sitl_log("healthy");
        let audit = audit_bytes(&bytes);
        assert_eq!(audit["health"]["status"], json!("PASS"), "{audit}");
        assert_eq!(audit["verdict"]["issues"], json!([]), "{audit}");
        // EKF3's one in-flight yaw alignment after takeoff is not a reset.
        assert!(audit["navigation"]["ekf_yaw_resets"].as_u64().unwrap() >= 1, "{}", audit["navigation"]);
        assert!(audit["navigation"]["ekf_yaw_alignments"].as_u64().unwrap() >= 1, "{}", audit["navigation"]);
        let filter = filter_review_bytes(&bytes, None, None, None);
        assert_eq!(filter["needs_update"], json!(false), "{filter}");
        assert!(filter["harmonics"].as_array().unwrap().iter().all(|row| row["notched"] == json!(true)), "{}", filter["harmonics"]);
        let motors = motor_balance_bytes(&bytes, None, None);
        assert_eq!(motors["issues"], json!([]), "{motors}");
        assert_eq!(motors["balance"]["status"], json!("balanced"), "{motors}");
    }

    #[test]
    fn sitl_audit_reads_heavy_vibration() {
        // SIM_VIB_FREQ near 95 Hz with a 50 m/s/s amplitude on both IMUs.
        let res = audit_bytes(&sitl_log("vibration"));
        assert_ne!(res["health"]["categories"]["vibrations"]["status"], json!("PASS"), "{res}");
        let peak = ["vibe_x_max", "vibe_y_max", "vibe_z_max"].iter().map(|k| res["vibrations"][*k].as_f64().unwrap()).fold(0.0, f64::max);
        assert!(peak > 30.0, "{}", res["vibrations"]);
    }

    #[test]
    fn sitl_audit_reads_a_motor_loss_as_lost_control() {
        // Motor 1 stops in a hover. SITL levels the vehicle on the ground, so no crash check fires:
        // the fall itself is ATT far from its desired attitude, and the motors at full output.
        let res = audit_bytes(&sitl_log("crash"));
        assert_eq!(res["health"]["categories"]["crash"]["status"], json!("FAIL"), "{res}");
        assert_eq!(res["health"]["status"], json!("FAIL"), "{res}");
        assert!(res["verdict"]["issues"].as_array().unwrap().iter().any(|i| i["message"].as_str().unwrap().starts_with("Attitude control lost")), "{res}");
        assert!(res["motors"]["saturation_count"].as_u64().unwrap() > 0, "{res}");
    }

    #[test]
    fn sitl_pid_review_reads_system_id_chirps() {
        // ArduCopter's System ID mode on the roll, then the pitch angle input, from Stabilize: the
        // indoor test. It gives what the stick doublets give.
        let chirp = sitl_log("sysid");
        let doublets = sitl_log("pid");
        for axis in ["roll", "pitch"] {
            let a = pid_review_bytes(&chirp, Some(axis), None, None);
            let b = pid_review_bytes(&doublets, Some(axis), None, None);
            let rise = |r: &Value| r["metrics"]["rise_time_ms"].as_f64().unwrap();
            let pm = |r: &Value| r["metrics"]["phase_margin_deg"].as_f64().unwrap();
            assert!((rise(&a) - rise(&b)).abs() < 5.0, "{axis}: {} vs {}", a["metrics"], b["metrics"]);
            assert!((pm(&a) - pm(&b)).abs() < 8.0, "{axis}: {} vs {}", a["metrics"], b["metrics"]);
            assert_eq!(a["logging"]["attitude_fast"], json!(true), "{}", a["logging"]);
            assert_eq!(a["logging"]["pid"], json!(true), "{}", a["logging"]);
            assert!(a["logging"]["firmware"].as_str().unwrap().starts_with("ArduCopter"), "{}", a["logging"]);
            assert_eq!(a["logging"]["bits_set"], json!([0, 1, 2, 12]), "{}", a["logging"]);
            assert_eq!(a["logging"]["gaps"], json!(0), "{}", a["logging"]);
            // The two chirps: roll (SID_AXIS 1), then pitch (SID_AXIS 2), each with its bounds.
            let runs = a["system_id_runs"].as_array().unwrap();
            assert_eq!(runs.iter().map(|r| r["axis"].as_str().unwrap()).collect::<Vec<_>>(), ["roll", "pitch"], "{runs:?}");
            assert!(runs.iter().all(|r| r["end_us"].as_u64().unwrap() > r["start_us"].as_u64().unwrap()), "{runs:?}");
            // SITL's rate loops track cleanly: nothing holds as an oscillation.
            assert!(a["diagnosis"].as_array().unwrap().iter().all(|d| !d.as_str().unwrap().contains("oscillation")), "{}", a["diagnosis"]);
            assert!(a["logging"]["docs"]["LOG_BITMASK"].as_str().unwrap().contains("/copter/docs/parameters.html#log-bitmask"), "{}", a["logging"]);
        }
    }

    #[test]
    fn sitl_pid_review_reads_the_fixed_wing_loops() {
        // ArduPlane in FBWA with aileron doublets; LOG_BITMASK bit 20 writes the loops at full rate.
        let res = pid_review_bytes(&sitl_log("plane_pid"), Some("roll"), None, None);
        assert_eq!(res["ok"], json!(true), "{res}");
        assert_eq!(res["controller"], json!("fixed wing"), "{res}");
        assert!((100.0..=600.0).contains(&res["metrics"]["rise_time_ms"].as_f64().unwrap()), "{}", res["metrics"]);
        assert_eq!(res["logging"]["attitude_fullrate"], json!(true), "{}", res["logging"]);
        assert!(res["logging"].get("pid").is_none(), "{}", res["logging"]);
        assert!(res["logging"]["firmware"].as_str().unwrap().starts_with("ArduPlane"), "{}", res["logging"]);
        assert!(res["logging"]["docs"]["LOG_BITMASK"].as_str().unwrap().contains("/plane/docs/"), "{}", res["logging"]);
    }

    #[test]
    fn filter_review_simulates_notch_attenuation_and_phase_delay() {
        let (gain_center, _) = notch_response(150.0, 50.0, 1000.0, 150.0);
        assert!(gain_center < 0.05, "Notch center gain should be deep null, was {gain_center}");

        let (gain_far, _) = notch_response(150.0, 50.0, 1000.0, 30.0);
        assert!((gain_far - 1.0).abs() < 0.25, "Passband gain should be near 1.0, was {gain_far}");

        // A motor line at 100 Hz with loud 2nd and 3rd harmonics on a quiet floor.
        let spectrum: Vec<(f64, f64)> = (20..1000).map(|f| {
            let f = f as f64;
            let amp = if [100.0, 200.0, 300.0].iter().any(|h| (f - h).abs() < 4.0) { 10.0 } else { 0.01 };
            (f, amp)
        }).collect();
        let plan = |hmncs: u32, lpf: f64| NotchPlan { enable: true, mode: 3, freq: 80.0, bw: 40.0, att: 40.0, hmncs, opts: 0, lpf };
        let two = plan(3, 40.0);
        let three = plan(7, 40.0);
        let two_centres = notch_centres(&two, 100.0, &[]);
        let three_centres = notch_centres(&three, 100.0, &[]);
        let now = plan_result(&two, &two_centres, 2000.0, &spectrum);
        assert!(now.noise_attenuation_db > 10.0, "{}", now.noise_attenuation_db);
        assert!(now.phase_delay_at_30hz_ms < 8.0, "{}", now.phase_delay_at_30hz_ms);
        // Notching the 3rd harmonic leaves less noise, so the low-pass can rise with the noise no worse.
        let left = residual_power(&two, &two_centres, 40.0, 2000.0, &spectrum);
        assert!(residual_power(&three, &three_centres, 40.0, 2000.0, &spectrum) < left);
        assert!([45.0, 50.0, 55.0, 60.0].iter().any(|lpf| residual_power(&three, &three_centres, *lpf, 2000.0, &spectrum) <= left));
        // A higher low-pass is less delay at 30 Hz.
        let raised = plan_result(&plan(7, 60.0), &three_centres, 2000.0, &spectrum);
        assert!(raised.phase_delay_at_30hz_ms < plan_result(&three, &three_centres, 2000.0, &spectrum).phase_delay_at_30hz_ms);
    }

    /// Target: pseudo-random holds of 0.1 to 0.4 s. Actual: `plant` applied to it sample by sample.
    fn loop_log(message: &str, labels: &str, hz: f64, seconds: f64, mut plant: impl FnMut(f64) -> f64) -> Vec<u8> {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 129, message, "Qff", labels);
        let mut seed = 12345u32;
        let mut next = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f64 / (1u32 << 24) as f64
        };
        let mut target = 0.0;
        let mut hold = 0usize;
        for i in 0..(hz * seconds) as usize {
            if hold == 0 {
                target = (next() - 0.5) * 120.0;
                hold = (((0.1 + 0.3 * next()) * hz) as usize).max(1);
            }
            hold -= 1;
            let actual = plant(target);
            rate_row(&mut buf, 1_000_000 + (i as f64 * 1e6 / hz) as u64, target as f32, actual as f32);
        }
        buf
    }

    /// A second-order loop: natural frequency `wn_hz`, damping `zeta`.
    fn second_order(hz: f64, wn_hz: f64, zeta: f64) -> impl FnMut(f64) -> f64 {
        let (dt, wn) = (1.0 / hz, 2.0 * std::f64::consts::PI * wn_hz);
        let (mut y, mut v) = (0.0f64, 0.0f64);
        move |target| {
            let out = y;
            for _ in 0..8 {
                let a = wn * wn * (target - y) - 2.0 * zeta * wn * v;
                v += a * dt / 8.0;
                y += v * dt / 8.0;
            }
            out
        }
    }

    #[test]
    fn pid_review_recovers_the_step_response_of_a_known_loop() {
        // zeta 0.4 overshoots by exp(-pi*zeta/sqrt(1-zeta^2)) = 25.4 %.
        let buf = loop_log("RATE", "TimeUS,RDes,R", 400.0, 60.0, second_order(400.0, 8.0, 0.4));
        let out = pid_review_bytes(&buf, Some("roll"), None, None);
        assert_eq!(out["ok"], json!(true), "{out}");
        assert_eq!(out["source"], json!("RATE"));
        let step = &out["step_response"];
        let overshoot = step["overshoot_pct"].as_f64().unwrap();
        assert!((19.0..=32.0).contains(&overshoot), "{step}");
        assert!((step["steady_state"].as_f64().unwrap() - 1.0).abs() < 0.05, "{step}");
        let rise = step["rise_time_ms"].as_f64().unwrap();
        assert!((15.0..=40.0).contains(&rise), "{step}");
        assert!(out["metrics"]["bandwidth_hz"].as_f64().unwrap() > 8.0, "{out}");
        // RATE's desired is before the target filter: no open-loop margin is claimed.
        assert!(out["metrics"]["phase_margin_deg"].is_null());
        assert!(out["margins_unavailable"].as_str().unwrap().contains("PIDR"));
        // No parameters in the log: none are reported, and none are proposed.
        assert_eq!(out["current_params"], json!({}));
        assert_eq!(out["proposed_params"], json!({}));

        let damped = loop_log("RATE", "TimeUS,RDes,R", 400.0, 60.0, second_order(400.0, 8.0, 1.0));
        let calm = pid_review_bytes(&damped, Some("roll"), None, None);
        assert!(calm["step_response"]["overshoot_pct"].as_f64().unwrap() < 5.0, "{calm}");
    }

    #[test]
    fn pid_review_reads_margins_from_the_pid_message() {
        // Open loop K/s with a delay of four samples: crossover at 5 Hz.
        let (hz, k) = (400.0, 2.0 * std::f64::consts::PI * 5.0);
        let mut actual = 0.0f64;
        let mut errors = std::collections::VecDeque::from(vec![0.0f64; 4]);
        let buf = loop_log("PIDR", "TimeUS,Tar,Act", hz, 90.0, move |target| {
            let out = actual;
            errors.push_back(target - actual);
            actual += k * errors.pop_front().unwrap() / hz;
            out
        });
        let out = pid_review_bytes(&buf, Some("roll"), None, None);
        assert_eq!(out["ok"], json!(true), "{out}");
        assert_eq!(out["source"], json!("PIDR"));
        assert!(out["margins_unavailable"].is_null(), "{out}");
        let m = &out["metrics"];
        let crossover = m["gain_crossover_hz"].as_f64().unwrap();
        assert!((4.0..=6.0).contains(&crossover), "{m}");
        // 90 degrees less the delay at 5 Hz: about 70.
        let pm = m["phase_margin_deg"].as_f64().unwrap();
        assert!((60.0..=80.0).contains(&pm), "{m}");
        let gm = m["gain_margin_db"].as_f64().unwrap();
        assert!((9.0..=17.0).contains(&gm), "{m}");
    }

    #[test]
    fn pid_review_does_not_invent_a_response_the_log_cannot_hold() {
        let slow = loop_log("RATE", "TimeUS,RDes,R", 10.0, 120.0, second_order(10.0, 1.0, 0.7));
        let out = pid_review_bytes(&slow, Some("roll"), None, None);
        assert_eq!(out["ok"], json!(true), "{out}");
        assert!(out["step_response"].is_null(), "{out}");
        assert!(out["step_response_unavailable"].as_str().unwrap().contains("10 Hz"), "{out}");
        assert!(out["metrics"]["overshoot_pct"].is_null());
        assert_eq!(out["proposed_params"], json!({}));

        // Actual is noise that ignores the target.
        let mut seed = 7u32;
        let deaf = loop_log("RATE", "TimeUS,RDes,R", 400.0, 30.0, move |_| {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            ((seed >> 8) as f64 / (1u32 << 24) as f64 - 0.5) * 40.0
        });
        let out = pid_review_bytes(&deaf, Some("roll"), None, None);
        assert!(out["step_response"].is_null(), "{out}");

        let short = loop_log("RATE", "TimeUS,RDes,R", 400.0, 0.5, second_order(400.0, 8.0, 0.4));
        assert_eq!(pid_review_bytes(&short, Some("roll"), None, None)["ok"], json!(false));
    }

    /// An angle loop around the rate loop, with a disturbance on the rate. `stick` false is a hover:
    /// the desired angle stays at zero and the rate target only reacts to the disturbance.
    fn outer_loop_log(stick: bool) -> Vec<u8> {
        let hz = 400.0;
        let mut buf = Vec::new();
        push_fmt(&mut buf, 129, "RATE", "Qff", "TimeUS,RDes,R");
        push_fmt(&mut buf, 130, "ATT", "Qf", "TimeUS,DesRoll");
        let mut seed = 99u32;
        let mut next = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f64 / (1u32 << 24) as f64
        };
        let mut plant = second_order(hz, 8.0, 0.4);
        let (mut desired, mut angle, mut gust, mut hold) = (0.0f64, 0.0f64, 0.0f64, 0usize);
        for i in 0..(hz * 90.0) as usize {
            if stick && hold == 0 {
                desired = (next() - 0.5) * 40.0;
                hold = ((0.3 + 0.7 * next()) * hz) as usize;
            }
            hold = hold.saturating_sub(1);
            let target = 6.0 * (desired - angle);
            gust = 0.97 * gust + (next() - 0.5) * 6.0;
            let actual = plant(target) + gust;
            angle += actual / hz;
            let us = 1_000_000 + (i as f64 * 1e6 / hz) as u64;
            rate_row(&mut buf, us, target as f32, actual as f32);
            buf.extend_from_slice(&[HEAD1, HEAD2, 130]);
            buf.extend_from_slice(&(us + 20).to_le_bytes());
            buf.extend_from_slice(&(desired as f32).to_le_bytes());
        }
        buf
    }

    #[test]
    fn pid_review_ties_the_estimate_to_the_desired_angle() {
        let out = pid_review_bytes(&outer_loop_log(true), Some("roll"), None, None);
        assert_eq!(out["ok"], json!(true), "{out}");
        assert_eq!(out["reference"], json!("ATT.DesRoll"));
        assert!(out["not_measured"].is_null(), "{out}");
        let step = &out["step_response"];
        let overshoot = step["overshoot_pct"].as_f64().unwrap();
        assert!((17.0..=34.0).contains(&overshoot), "{step}");
        assert!((step["steady_state"].as_f64().unwrap() - 1.0).abs() < 0.08, "{step}");

        // A hover: the target only follows the vehicle. Nothing is measured, and nothing is proposed.
        let hover = pid_review_bytes(&outer_loop_log(false), Some("roll"), None, None);
        assert_eq!(hover["ok"], json!(true), "{hover}");
        assert!(hover["not_measured"].is_string(), "{hover}");
        assert!(hover["step_response"].is_null(), "{hover}");
        assert!(hover["metrics"]["bandwidth_hz"].is_null(), "{hover}");
        assert_eq!(hover["frequency_response"], json!([]));
        assert_eq!(hover["proposed_params"], json!({}));
        assert_eq!(pid_picture_bytes(&outer_loop_log(false), &json!({ "axis": "roll" }))["ok"], json!(false));
    }

    #[test]
    fn pid_review_rejects_a_target_that_follows_the_vehicle() {
        // The same hover without ATT: the target itself is the reference, and the estimate is the
        // outer loop backwards, gain rising with frequency at -90°.
        let mut buf = Vec::new();
        push_fmt(&mut buf, 129, "RATE", "Qff", "TimeUS,RDes,R");
        let hz = 400.0;
        let mut seed = 5u32;
        let (mut angle, mut gust) = (0.0f64, 0.0f64);
        let mut plant = second_order(hz, 8.0, 0.4);
        for i in 0..(hz * 60.0) as usize {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            gust = 0.97 * gust + ((seed >> 8) as f64 / (1u32 << 24) as f64 - 0.5) * 6.0;
            let target = -6.0 * angle;
            let actual = plant(target) + gust;
            angle += actual / hz;
            rate_row(&mut buf, 1_000_000 + (i as f64 * 1e6 / hz) as u64, target as f32, actual as f32);
        }
        let out = pid_review_bytes(&buf, Some("roll"), None, None);
        assert_eq!(out["ok"], json!(true), "{out}");
        assert!(out["reference"].is_null());
        assert!(out["not_measured"].is_string(), "{out}");
        assert!(out["step_response"].is_null(), "{out}");
        assert_eq!(out["proposed_params"], json!({}));

        // frf is the same estimate, and it gives the same answer for the same pair.
        let frf = frf_bytes(&buf, "RATE", "R", None, None);
        assert_eq!(frf["ok"], json!(true), "{frf}");
        assert!(frf["not_measured"].is_string(), "{frf}");
        assert_eq!(frf["bins"], json!([]));
        assert!(frf["bandwidth_hz"].is_null());
    }

    fn pid_parm(buf: &mut Vec<u8>, name: &str, value: f32) {
        buf.extend_from_slice(&[HEAD1, HEAD2, 140]);
        buf.extend_from_slice(&1000u64.to_le_bytes());
        let mut bytes = [0u8; 16];
        bytes[..name.len()].copy_from_slice(name.as_bytes());
        buf.extend_from_slice(&bytes);
        buf.extend_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn pid_review_reads_the_loops_of_a_plane_and_a_quadplane() {
        // A plane names its roll rate gains RLL_RATE_ and logs the loop in PIDR.
        let mut plane = loop_log("PIDR", "TimeUS,Tar,Act", 50.0, 240.0, second_order(50.0, 2.0, 0.5));
        push_fmt(&mut plane, 140, "PARM", "QNf", "TimeUS,Name,Value");
        pid_parm(&mut plane, "RLL_RATE_P", 0.08);
        pid_parm(&mut plane, "RLL_RATE_FF", 0.35);
        let out = pid_review_bytes(&plane, Some("roll"), None, None);
        assert_eq!(out["ok"], json!(true), "{out}");
        assert_eq!(out["controller"], json!("fixed wing"));
        assert!(out["current_params"]["RLL_RATE_P"].is_number(), "{out}");
        assert!(out["current_params"].get("ATC_RAT_RLL_P").is_none());
        // zeta 0.5 overshoots by 16 %. The step is read over one second on a wing.
        let step = &out["step_response"];
        assert!((8.0..=25.0).contains(&step["overshoot_pct"].as_f64().unwrap()), "{out}");
        assert!(step["points"].as_array().unwrap().last().unwrap()["t_ms"].as_f64().unwrap() > 900.0, "{step}");
        // No gain change is proposed for a fixed-wing loop, and no open-loop margin is claimed.
        assert_eq!(out["proposed_params"], json!({}));
        assert!(out["metrics"]["phase_margin_deg"].is_null());

        // A quadplane holds both. Its VTOL loop is in PIQR, with the Q_A_RAT_ gains.
        let mut quad = loop_log("PIQR", "TimeUS,Tar,Act", 400.0, 60.0, second_order(400.0, 8.0, 0.4));
        push_fmt(&mut quad, 140, "PARM", "QNf", "TimeUS,Name,Value");
        pid_parm(&mut quad, "RLL_RATE_P", 0.08);
        pid_parm(&mut quad, "Q_A_RAT_RLL_P", 0.12);
        let out = pid_review_bytes(&quad, Some("roll"), None, None);
        assert_eq!(out["ok"], json!(true), "{out}");
        assert_eq!(out["controller"], json!("quadplane VTOL"));
        assert_eq!(out["source"], json!("PIQR"));
        assert!(out["current_params"]["Q_A_RAT_RLL_P"].is_number(), "{out}");
    }

    #[test]
    fn filter_review_answers_when_the_log_has_no_gyro_batches() {
        // RATE alone, with a row count that is not a power of two. This once stopped the tool.
        let mut slow = Vec::new();
        push_fmt(&mut slow, 129, "RATE", "Qff", "TimeUS,RDes,R");
        for i in 0..100u64 {
            rate_row(&mut slow, 1_000_000 + i * 100_000, 0.0, (i as f32 * 0.7).sin());
        }
        let out = filter_review_bytes(&slow, None, None, None);
        assert_eq!(out["ok"], json!(false), "{out}");
        assert!(out["error"].as_str().unwrap().contains("10 Hz"), "{out}");

        // RATE at 400 Hz with an 80 Hz line is enough for a spectrum.
        let mut fast = Vec::new();
        push_fmt(&mut fast, 129, "RATE", "Qff", "TimeUS,RDes,R");
        for i in 0..1000u64 {
            let t = i as f64 / 400.0;
            rate_row(&mut fast, 1_000_000 + i * 2_500, 0.0, (2.0 * std::f64::consts::PI * 80.0 * t).sin() as f32);
        }
        let out = filter_review_bytes(&fast, None, None, None);
        assert_eq!(out["ok"], json!(true), "{out}");

        let empty = filter_review_bytes(&slow[..89], None, None, None);
        assert_eq!(empty["ok"], json!(false), "{empty}");
    }

    #[test]
    fn pid_response_charts_are_step_and_bode() {
        let buf = loop_log("RATE", "TimeUS,RDes,R", 400.0, 60.0, second_order(400.0, 8.0, 0.4));
        let step = pid_picture_bytes(&buf, &json!({ "axis": "roll", "slug": "roll-step" }));
        assert_eq!(step["ok"], json!(true), "{step}");
        assert_eq!(step["chart"]["x_label"], json!("ms"));
        assert_eq!(step["chart"]["slug"], json!("roll-step"));
        let names: Vec<&str> = step["chart"]["axes"][0]["lines"].as_array().unwrap().iter().map(|line| line["name"].as_str().unwrap()).collect();
        assert_eq!(&names[..5], &["target −10 %", "target +10 %", "roll −1σ", "roll +1σ", "target"]);
        // The legend carries the overshoot and the rise time.
        assert!(names[5].starts_with("roll · +") && names[5].ends_with(" ms"), "{names:?}");
        let lines = step["chart"]["axes"][0]["lines"].as_array().unwrap();
        assert_eq!(lines[1]["fill"], json!(true));
        assert_eq!(lines[3]["quiet"], json!(true));
        assert_eq!(step["axes"][0]["established"], json!(true), "{step}");
        assert!(lines[5].get("dash").is_none());
        assert!(step["axes"][0]["overshoot_pct"].as_f64().unwrap() > 15.0, "{step}");

        // RATE in this log has no pitch or yaw columns: all draws roll and names the rest.
        let all = pid_picture_bytes(&buf, &json!({}));
        assert_eq!(all["axes"].as_array().unwrap().len(), 1, "{all}");
        assert_eq!(all["missing"].as_array().unwrap().len(), 2, "{all}");

        let bode = pid_picture_bytes(&buf, &json!({ "kind": "bode", "axis": "roll" }));
        assert_eq!(bode["ok"], json!(true), "{bode}");
        assert_eq!(bode["chart"]["x_log"], json!(true));
        assert_eq!(bode["chart"]["stack"], json!(true));
        let gain = bode["chart"]["axes"][0]["lines"].as_array().unwrap();
        assert_eq!(gain[2]["name"], json!("target"));
        // zeta 0.4 has a resonance of about +2.7 dB below its natural frequency of 8 Hz.
        let label = gain[3]["name"].as_str().unwrap();
        assert!(label.starts_with("roll · peak +") && label.contains(" · −3 dB at "), "{label}");
        let peak = bode["axes"][0]["peak_gain_db"].as_f64().unwrap();
        assert!((1.5..=4.0).contains(&peak), "{bode}");
        assert_eq!(bode["chart"]["axes"][1]["lines"][0]["name"], json!("roll phase"));
        assert_eq!(bode["chart"]["axes"][1]["dtick"], json!(45));
        // The line stops where the measurement stops.
        let x = gain[3]["x"].as_array().unwrap();
        assert!(gain[3]["y"].as_array().unwrap().last().unwrap().is_number());
        assert_eq!(gain[0]["x"][1], x[x.len() - 1]);
        assert!(bode["axes"][0]["bandwidth_hz"].as_f64().unwrap() > 8.0, "{bode}");

        assert_eq!(pid_picture_bytes(&buf, &json!({ "kind": "nyquist" }))["ok"], json!(false));
        assert_eq!(pid_picture_bytes(&buf, &json!({ "axis": "pitch" }))["ok"], json!(false));
    }

    #[test]
    fn pid_review_runs_on_real_logs_if_present() {
        // ARDULOOPS_PID_LOGS names a folder of folders of .bin files. Without it the test does nothing.
        let Some(dir) = std::env::var_os("ARDULOOPS_PID_LOGS") else { return };
        let Ok(cases) = std::fs::read_dir(dir) else { return };
        for case in cases.flatten() {
            let Ok(files) = std::fs::read_dir(case.path()) else { continue };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) != Some("bin") {
                    continue;
                }
                let bytes = std::fs::read(&path).unwrap();
                for axis in ["roll", "pitch", "yaw"] {
                    // ARDULOOPS_PID_START_S and ARDULOOPS_PID_END_S bound the interval, in seconds of TimeUS.
                    let bound = |var: &str| std::env::var(var).ok().and_then(|v| v.parse::<f64>().ok()).map(|s| (s * 1e6) as u64);
                    let res = pid_review_bytes(&bytes, Some(axis), bound("ARDULOOPS_PID_START_S"), bound("ARDULOOPS_PID_END_S"));
                    let mut brief = res.clone();
                    if let Some(obj) = brief.as_object_mut() {
                        for key in ["note", "suggested_chart", "key_intervals", "best_step_interval", "frequency_response"] {
                            obj.remove(key);
                        }
                    }
                    if let Some(step) = brief.get_mut("step_response").and_then(Value::as_object_mut) {
                        step.remove("points");
                    }
                    println!("REAL {} {axis}: {brief}", path.file_name().unwrap().to_string_lossy());
                    // A margin is either measured or null, never a placeholder.
                    if res["ok"] == json!(true) && res["source"] == json!("RATE") {
                        assert!(res["metrics"]["phase_margin_deg"].is_null(), "{res}");
                    }
                }
            }
        }
    }

    fn arm_row(buf: &mut Vec<u8>, us: u64, armed: u8) {
        buf.extend_from_slice(&[HEAD1, HEAD2, 10]);
        buf.extend_from_slice(&us.to_le_bytes());
        buf.push(armed);
        buf.extend_from_slice(&0u32.to_le_bytes());
    }

    fn err_row(buf: &mut Vec<u8>, us: u64, subsystem: u8, code: u8) {
        buf.extend_from_slice(&[HEAD1, HEAD2, 11, ]);
        buf.extend_from_slice(&us.to_le_bytes());
        buf.extend_from_slice(&[subsystem, code]);
    }

    fn gps_row(buf: &mut Vec<u8>, us: u64, lat: f64, lng: f64, week: u16) {
        buf.extend_from_slice(&[HEAD1, HEAD2, 12]);
        buf.extend_from_slice(&us.to_le_bytes());
        buf.extend_from_slice(&((lat * 1e7).round() as i32).to_le_bytes());
        buf.extend_from_slice(&((lng * 1e7).round() as i32).to_le_bytes());
        buf.extend_from_slice(&week.to_le_bytes());
        buf.extend_from_slice(&1000u32.to_le_bytes());
    }

    #[test]
    fn audit_reads_the_board_voltage_from_powr() {
        // Without POWR the board voltage is not measured, and no value stands in for it.
        let bare = audit_bytes(&flight_log());
        assert_eq!(bare["ok"], json!(true), "{bare}");
        assert!(bare["power"]["vcc_min"].is_null(), "{bare}");
        assert_eq!(bare["power"]["status"], json!("not measured"));

        let mut buf = flight_log();
        push_fmt(&mut buf, 60, "POWR", "QffHHB", "TimeUS,Vcc,VServo,Flags,AccFlags,Safety");
        for (i, vcc) in [5.02f32, 4.98, 4.52, 5.0].into_iter().enumerate() {
            buf.extend_from_slice(&[HEAD1, HEAD2, 60]);
            buf.extend_from_slice(&(2_000_000 + i as u64 * 100_000).to_le_bytes());
            buf.extend_from_slice(&vcc.to_le_bytes());
            buf.extend_from_slice(&5.0f32.to_le_bytes());
            buf.extend_from_slice(&[0, 0, 0, 0, 0]);
        }
        let out = audit_bytes(&buf);
        assert_eq!(out["power"]["vcc_min"], json!(4.52), "{out}");
        assert_eq!(out["power"]["status"], json!("critical"));
        let issues = out["verdict"]["issues"].as_array().unwrap();
        assert!(issues.iter().any(|issue| issue["category"] == json!("power") && issue["severity"] == json!("critical")), "{out}");
    }

    fn flight_log() -> Vec<u8> {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 10, "ARM", "QBI", "TimeUS,ArmState,ArmChecks");
        push_fmt(&mut buf, 11, "ERR", "QBB", "TimeUS,Subsys,ECode");
        push_fmt(&mut buf, 12, "GPS", "QLLHI", "TimeUS,Lat,Lng,GWk,GMS");
        push_fmt(&mut buf, 13, "MSG", "QZ", "TimeUS,Message");
        push_qz(&mut buf, 13, 500_000, "ArduCopter V4.8.0-dev (113108bd)");
        push_qz(&mut buf, 13, 600_000, "DAKEFPVH743 004B0039 30365107 33363732");
        gps_row(&mut buf, 900_000, 50.45, 30.52, 2390);
        arm_row(&mut buf, 1_000_000, 1);
        gps_row(&mut buf, 2_000_000, 50.45 + 0.001, 30.52 + 0.001, 2390);
        err_row(&mut buf, 3_000_000, 6, 1);
        err_row(&mut buf, 3_500_000, 12, 1);
        err_row(&mut buf, 3_600_000, 16, 0);
        arm_row(&mut buf, 5_000_000, 0);
        buf
    }

    #[test]
    fn scan_counts_armed_time_flights_and_flags() {
        let (scan, _) = scan_bytes(&flight_log());
        assert_eq!(scan["armed_s"], json!(4.0), "{scan}");
        assert_eq!(scan["flights"], json!(1));
        let kinds: Vec<_> = scan["flags"].as_array().unwrap().iter().map(|f| f["kind"].as_str().unwrap().to_string()).collect();
        assert_eq!(kinds, vec!["crash", "batt"], "a resolved ERR (code 0) is not a flag");
    }

    #[test]
    fn a_boot_without_arming_is_never_armed() {
        let mut buf = Vec::new();
        push_fmt(&mut buf, 10, "ARM", "QBI", "TimeUS,ArmState,ArmChecks");
        push_fmt(&mut buf, 13, "MSG", "QZ", "TimeUS,Message");
        push_qz(&mut buf, 13, 500_000, "ArduCopter V4.8.0-dev (113108bd)");
        let (scan, _) = scan_bytes(&buf);
        assert_eq!(scan["flights"], json!(0));
        assert_eq!(scan["armed_s"], json!(0.0));
    }

    #[test]
    fn values_the_firmware_writes_at_boot_are_not_changes() {
        let before: BTreeMap<String, f64> = [("ATC_RAT_RLL_P".to_string(), 0.135), ("BARO1_GND_PRESS".to_string(), 101287.0), ("STAT_BOOTCNT".to_string(), 4.0)].into();
        let after: BTreeMap<String, f64> = [("ATC_RAT_RLL_P".to_string(), 0.15), ("BARO1_GND_PRESS".to_string(), 101324.0), ("STAT_BOOTCNT".to_string(), 5.0)].into();
        let changes = param_changes(&before, &after);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0]["name"], json!("ATC_RAT_RLL_P"));
    }

    #[test]
    fn share_moves_home_to_zero_shifts_the_date_and_clears_the_serial() {
        let original = flight_log();
        let mut copy = original.clone();
        anonymise(&mut copy, &ShareOptions { location: true, date: true, board: true, messages: false });
        assert_eq!(copy.len(), original.len(), "fields are rewritten in place");
        let formats = parse_formats(&copy);
        let mut fixes = Vec::new();
        let mut texts = Vec::new();
        let mut incomplete = false;
        walk(&copy, &formats, |fmt, _, values| {
            if fmt.name == "GPS" {
                fixes.push((field_f64(fmt, values, "Lat").unwrap(), field_f64(fmt, values, "Lng").unwrap(), field_u64(fmt, values, "GWk").unwrap()));
            }
            if fmt.name == "MSG" {
                texts.push(field_str(fmt, values, "Message").unwrap_or_default());
            }
        }, &mut incomplete);
        assert_eq!((fixes[0].0, fixes[0].1), (0.0, 0.0), "home is at 0 N 0 E");
        assert!((fixes[1].0 - 0.001).abs() < 1e-6, "north offset kept");
        let east = 0.001 * 50.45f64.to_radians().cos();
        assert!((fixes[1].1 - east).abs() < 1e-6, "east offset kept in metres");
        assert_eq!(fixes[0].2, SHARE_WEEK);
        assert_eq!(texts[0], "ArduCopter V4.8.0-dev (113108bd)", "the firmware line stays");
        assert_eq!(texts[1], "DAKEFPVH743 00000000 00000000 00000000");
    }
}
