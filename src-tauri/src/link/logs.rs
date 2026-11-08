// On-board log list and download.


fn request_log_list(conn: &dyn MavConnection<MavMessage>, st: &mut LinkState) {
    st.sample.logs.clear();
    st.sample.log_list_expected = None;
    st.sample.log_list_at = wall_time();
    send_msg(
        conn,
        &MavMessage::LOG_REQUEST_LIST(LOG_REQUEST_LIST_DATA {
            start: 0,
            end: u16::MAX,
            target_system: st.target_system,
            target_component: st.target_component,
        }),
    );
}

fn end_log_request(conn: &dyn MavConnection<MavMessage>, st: &LinkState) {
    send_msg(
        conn,
        &MavMessage::LOG_REQUEST_END(LOG_REQUEST_END_DATA {
            target_system: st.target_system,
            target_component: st.target_component,
        }),
    );
}

fn request_log_range(
    conn: &dyn MavConnection<MavMessage>,
    st: &LinkState,
    id: u16,
    ofs: usize,
    count: usize,
) {
    send_msg(
        conn,
        &MavMessage::LOG_REQUEST_DATA(LOG_REQUEST_DATA_DATA {
            ofs: ofs.min(u32::MAX as usize) as u32,
            count: count.min(u32::MAX as usize) as u32,
            id,
            target_system: st.target_system,
            target_component: st.target_component,
        }),
    );
}

fn log_bit_received(bits: &[u64], index: usize) -> bool {
    bits.get(index / 64)
        .is_some_and(|word| (word & (1_u64 << (index % 64))) != 0)
}

fn mark_log_bit(bits: &mut [u64], index: usize) -> bool {
    let Some(word) = bits.get_mut(index / 64) else {
        return false;
    };
    let mask = 1_u64 << (index % 64);
    let was_set = *word & mask != 0;
    *word |= mask;
    !was_set
}

fn update_log_download_report(st: &mut LinkState, error: String) {
    if st.active_log_download.as_ref().is_some_and(|active| active.peek_head > 0) {
        return;
    }
    if let Some(active) = &st.active_log_download {
        st.sample.log_download = Some(LogDownload {
            id: active.id,
            size: active.size as u32,
            received: active
                .received_chunks
                .saturating_mul(LOG_BLOCK)
                .min(active.size) as u32,
            complete: false,
            path: String::new(),
            error,
            updated_at: wall_time(),
        });
    }
}

fn log_chunk_wanted(active: &ActiveLogDownload, chunk: usize) -> bool {
    if active.peek_head == 0 {
        return true;
    }
    let start = chunk * LOG_BLOCK;
    if start < active.peek_head {
        return true;
    }
    if active.peek_tail == 0 {
        return false;
    }
    let tail_at = active.size.saturating_sub(active.peek_tail);
    start < active.size && start + LOG_BLOCK > tail_at
}

fn first_missing_log_chunk(active: &ActiveLogDownload) -> Option<usize> {
    let chunks = active.size.div_ceil(LOG_BLOCK);
    (0..chunks).find(|index| log_chunk_wanted(active, *index) && !log_bit_received(&active.received, *index))
}

fn log_download_complete(active: &ActiveLogDownload) -> bool {
    let chunks = active.size.div_ceil(LOG_BLOCK);
    chunks == 0
        || (0..chunks).all(|index| !log_chunk_wanted(active, index) || log_bit_received(&active.received, index))
}

/// Gaps closer than this (in chunks) go in one request: a few chunks sent twice cost less than
/// one more round trip on a radio link.
const LOG_GAP_MERGE: usize = 8;

fn next_log_request(active: &ActiveLogDownload) -> Option<(usize, usize)> {
    let chunk = first_missing_log_chunk(active)?;
    let offset = chunk * LOG_BLOCK;
    let chunks = active.size.div_ceil(LOG_BLOCK);
    let mut last_missing = chunk;
    let mut next = chunk + 1;
    while next < chunks && next - chunk < 512 && log_chunk_wanted(active, next) && next - last_missing <= LOG_GAP_MERGE {
        if !log_bit_received(&active.received, next) {
            last_missing = next;
        }
        next += 1;
    }
    let end = ((last_missing + 1) * LOG_BLOCK).min(active.size);
    Some((offset, end - offset))
}

/// Every chunk of the last request has arrived: the board has closed that transfer.
fn requested_range_done(active: &ActiveLogDownload) -> bool {
    let (start, end) = active.requested;
    end > start && (start / LOG_BLOCK..end.div_ceil(LOG_BLOCK)).all(|chunk| log_bit_received(&active.received, chunk))
}

fn write_log_bytes(active: &mut ActiveLogDownload, offset: usize, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    if active.peek_head == 0 {
        let end = (offset + data.len()).min(active.size).min(active.bytes.len());
        if end > offset {
            active.bytes[offset..end].copy_from_slice(&data[..end - offset]);
        }
        return;
    }
    if offset < active.peek_head {
        let n = data.len().min(active.peek_head - offset);
        active.bytes[offset..offset + n].copy_from_slice(&data[..n]);
    }
    if active.peek_tail == 0 {
        return;
    }
    let tail_at = active.size.saturating_sub(active.peek_tail);
    let overlap_start = offset.max(tail_at);
    let overlap_end = (offset + data.len()).min(active.size);
    if overlap_end <= overlap_start {
        return;
    }
    let src = overlap_start - offset;
    let dst = active.peek_head + (overlap_start - tail_at);
    let n = overlap_end - overlap_start;
    if dst + n <= active.bytes.len() {
        active.bytes[dst..dst + n].copy_from_slice(&data[src..src + n]);
    }
}

pub(crate) fn remember_brief(sample: &mut Sample, brief: LogBrief) {
    if let Some(slot) = sample.log_briefs.iter_mut().find(|item| item.id == brief.id) {
        *slot = brief;
    } else {
        sample.log_briefs.push(brief);
    }
}

fn start_text(summary_utc: &str, time_utc: u32) -> String {
    if !summary_utc.is_empty() {
        return summary_utc.to_string();
    }
    if time_utc == 0 {
        return String::new();
    }
    crate::dflog::unix_rfc3339(time_utc as u64)
}

/// LOG_DATA with count 0 at `offset`: the log ends there, whatever its listed size said. A
/// log that ends at 0 has no data (the board lists it but cannot read it); a shorter one is
/// kept up to that point and completes once the bytes before it are in.
fn log_data_ended(st: &mut LinkState, offset: usize) {
    let Some(active) = st.active_log_download.as_mut() else {
        return;
    };
    if offset >= active.size {
        return;
    }
    if offset == 0 {
        let active = st.active_log_download.take().expect("active log");
        let (id, size) = (active.id, active.size as u32);
        if active.peek_head > 0 {
            fail_log_peek(st, id, size, NO_LOG_DATA);
        } else {
            st.sample.log_download = Some(LogDownload {
                id,
                size,
                received: 0,
                complete: true,
                path: String::new(),
                error: NO_LOG_DATA.into(),
                updated_at: wall_time(),
            });
        }
        return;
    }
    active.size = offset;
    if active.peek_head > 0 {
        // The tail was placed from the listed end; only the head is still right.
        active.peek_head = active.peek_head.min(offset);
        active.peek_tail = 0;
        active.bytes.truncate(active.peek_head);
    } else {
        active.bytes.truncate(offset);
    }
    active.last_data = Instant::now();
    update_log_download_report(st, String::new());
}

const NO_LOG_DATA: &str = "The board has no data in this log";

fn finish_log_peek(st: &mut LinkState, active: ActiveLogDownload) {
    let head_len = active.peek_head.min(active.bytes.len());
    let summary = crate::dflog::summarize_log(&active.bytes[..head_len], &active.bytes[head_len..]);
    let time_utc = st
        .sample
        .logs
        .iter()
        .find(|log| log.id == active.id)
        .map(|log| log.time_utc)
        .unwrap_or(0);
    remember_brief(
        &mut st.sample,
        LogBrief {
            id: active.id,
            size: active.size as u32,
            firmware: summary.firmware,
            frame: summary.frame,
            start_utc: start_text(&summary.start_utc, time_utc),
            duration_us: summary.duration_us,
            error: String::new(),
            updated_at: wall_time(),
        },
    );
}

fn fail_log_peek(st: &mut LinkState, id: u16, size: u32, error: &str) {
    remember_brief(
        &mut st.sample,
        LogBrief {
            id,
            size,
            firmware: String::new(),
            frame: String::new(),
            start_utc: String::new(),
            duration_us: 0,
            error: error.to_string(),
            updated_at: wall_time(),
        },
    );
}

/// The linked vehicle is ArduPilot SITL: only SITL has the SIM_* parameters.
fn sitl_link(st: &LinkState) -> bool {
    st.sample.params.contains_key("SIM_SPEEDUP") || st.sample.params.contains_key("SIM_WIND_SPD")
}

/// `sitl-copter`, `sitl-plane`, or `sitl` before the vehicle type is known.
fn sitl_log_dir(frame: &str) -> String {
    if frame.is_empty() { "sitl".into() } else { format!("sitl-{frame}") }
}

/// The folder under logs/ for this board: its boot uid, or system-{id} before the boot banner is
/// seen. SITL has no board UID (its boot line is the machine id), so it goes by vehicle type,
/// apart from real boards: `sitl-copter`, `sitl-plane`.
fn log_dir_key(st: &LinkState) -> String {
    if sitl_link(st) {
        sitl_log_dir(&st.sample.frame)
    } else if st.sample.boot_uid.is_empty() {
        format!("system-{}", st.target_system)
    } else {
        st.sample.boot_uid.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
    }
}

fn save_completed_log(st: &mut LinkState, active: ActiveLogDownload) {
    // Before the boot line arrives a board is only system-<id>; the log itself names it.
    let key = if !sitl_link(st) && st.sample.boot_uid.is_empty() {
        Some(crate::dflog::vehicle_dir(&active.bytes)).filter(|dir| dir != "imported").unwrap_or_else(|| log_dir_key(st))
    } else {
        log_dir_key(st)
    };
    st.sample.log_dir = key.clone();
    let root = crate::db::data_dir().join("logs").join(key);
    let time_utc = st.sample.logs.iter().find(|log| log.id == active.id).map(|log| log.time_utc).unwrap_or(0);
    let path = root.join(crate::dflog::downloaded_log_name(active.id, &active.bytes, time_utc));
    let result = fs::create_dir_all(&root).and_then(|_| fs::write(&path, &active.bytes));
    let (complete, path, error) = match result {
        Ok(()) => (true, path.display().to_string(), String::new()),
        Err(error) => (true, String::new(), format!("Could not save log: {error}")),
    };
    st.sample.log_download = Some(LogDownload {
        id: active.id,
        size: active.size as u32,
        received: active.size as u32,
        complete,
        path,
        error,
        updated_at: wall_time(),
    });
}

/// A full file is paced near 6 KB/s on a slow serial link. The header peek stays short.
/// No log data for this long: the board may still hold the last transfer open.
const LOG_STALL: Duration = Duration::from_secs(3);

fn log_download_budget(size: usize, peek: bool) -> Duration {
    if peek {
        return Duration::from_secs(40);
    }
    Duration::from_secs((size as u64 / 6_000).clamp(180, 3_600) + 60)
}

fn drive_log_download(conn: &dyn MavConnection<MavMessage>, st: &mut LinkState) {
    let now = Instant::now();
    let complete = st
        .active_log_download
        .as_ref()
        .is_some_and(log_download_complete);
    if complete {
        let active = st.active_log_download.take().expect("active log");
        let peek = active.peek_head > 0;
        end_log_request(conn, st);
        if peek {
            finish_log_peek(st, active);
        } else {
            save_completed_log(st, active);
        }
        return;
    }
    let Some(active) = st.active_log_download.as_mut() else {
        return;
    };
    let limit = log_download_budget(active.size, active.peek_head > 0);
    if now.duration_since(active.started) > limit {
        let id = active.id;
        let size = active.size as u32;
        let received = active.received_chunks.saturating_mul(LOG_BLOCK).min(active.size) as u32;
        let peek = active.peek_head > 0;
        st.active_log_download = None;
        if peek {
            fail_log_peek(st, id, size, "Could not read the log header");
        } else {
            st.sample.log_download = Some(LogDownload {
                id,
                size,
                received,
                complete: true,
                path: String::new(),
                error: "Log download timed out".into(),
                updated_at: wall_time(),
            });
        }
        end_log_request(conn, st);
        return;
    }
    // The next gap goes out at once when the last request came in whole; otherwise after a
    // quiet spell, since the board ignores a request while its transfer is still open.
    let gap = if requested_range_done(active) { 30 } else if active.peek_head == 0 { 900 } else { 200 };
    if now.duration_since(active.last_data) < Duration::from_millis(gap)
        || now.duration_since(active.last_request) < Duration::from_millis(gap)
    {
        return;
    }
    let Some((offset, count)) = next_log_request(active) else {
        return;
    };
    let id = active.id;
    // ArduPilot drops a LOG_REQUEST_DATA while it holds a transfer it thinks is still open (a lost
    // LOG_REQUEST_END or end packet). After a quiet spell, end that transfer before asking again,
    // as Mission Planner does; otherwise every retry is ignored and the download stays put.
    let stalled = now.duration_since(active.last_data) > LOG_STALL;
    active.last_request = now;
    active.requested = (offset, offset + count);
    if stalled {
        end_log_request(conn, st);
    }
    request_log_range(conn, st, id, offset, count);
}
