use std::ops::Deref;
use std::sync::atomic::Ordering;

use super::{Align2, Arc, Color32, FilmcraftApp, ItemId, Pos2, Rect, Stroke, Tick, Tokens, TrackItem, WaveformStyle, pos2, vec2};

/// Precomputed min/max waveform peaks (one bucket per 256 samples at 48 kHz).
///
/// Stores up to two display channels (`L`/`R`) alongside a combined envelope across all
/// decoded channels so `Deref<Target = [(f32, f32)]>` always reflects audio present on any
/// channel.
#[derive(Clone, Debug)]
pub struct WaveformPeaks {
    pub channels: Vec<Vec<(f32, f32)>>,
    pub combined: Vec<(f32, f32)>,
    pub max_peak: f32,
}

impl WaveformPeaks {
    pub fn from_channels(channels: Vec<Vec<(f32, f32)>>) -> Self {
        let len = channels.iter().map(Vec::len).max().unwrap_or(0);
        let combined = if channels.len() <= 1 {
            channels.first().cloned().unwrap_or_default()
        } else {
            (0..len)
                .map(|i| {
                    channels.iter().fold((0.0f32, 0.0f32), |(lo, hi), ch| {
                        let (a, b) = ch.get(i).copied().unwrap_or((0.0, 0.0));
                        (lo.min(a), hi.max(b))
                    })
                })
                .collect()
        };
        let max_peak = combined.iter().fold(0.0f32, |m, &(a, b)| m.max(a.abs()).max(b.abs()));
        Self { channels, combined, max_peak }
    }
}

impl Deref for WaveformPeaks {
    type Target = [(f32, f32)];

    fn deref(&self) -> &Self::Target {
        &self.combined
    }
}

/// The height of a clip's waveform for a clip body `body_height` tall: either the full clip height
/// when `full_height` is enabled (or on compact tracks under 50 points), or the lower 42% under the
/// name strip.
pub(super) fn waveform_zone_height(body_height: f32, full_height: bool) -> f32 {
    if full_height || body_height < 50.0 { (body_height - 2.0).max(8.0) } else { (body_height * 0.42).max(8.0) }
}

/// Map a normalized linear amplitude (already scaled by clip gain and display normalization) into a
/// `[0.0, 1.0]` bar fraction according to logarithmic (`dynamic`) vs linear mode.
fn scale_amplitude(amp: f32, dynamic: bool) -> f32 {
    if dynamic {
        let db = 20.0 * amp.max(1e-5).log10();
        ((db + 48.0) / 48.0).clamp(0.0, 1.0)
    } else {
        amp.clamp(0.0, 1.0)
    }
}

/// Normalize `[s0, s1]` bucket bounds cleanly, handling both forward and reversed clips (`it.reverse`).
pub(super) fn bucket_range_for_pixel(it: &TrackItem, x: i32, body_min_x: f32, dur_px: f32, sr: f64, spp: f64) -> (usize, usize) {
    let f0 = (x as f32 - body_min_x) / dur_px;
    let f1 = (x as f32 + 1.0 - body_min_x) / dur_px;
    let t0 = it.start + Tick((it.duration.0 as f64 * f0 as f64) as i64);
    let t1 = it.start + Tick((it.duration.0 as f64 * f1 as f64) as i64);
    let a = (it.source_time_at(t0).seconds().max(0.0) * sr / spp) as usize;
    let b = (it.source_time_at(t1).seconds().max(0.0) * sr / spp) as usize;
    let lo = a.min(b);
    let hi = a.max(b).max(lo + 1);
    (lo, hi)
}

fn sample_channel_minmax(bins: &[(f32, f32)], s0: usize, s1: usize) -> (f32, f32) {
    if s0 >= bins.len() {
        return (0.0, 0.0);
    }
    let end = s1.min(bins.len());
    bins[s0..end].iter().fold((0.0f32, 0.0f32), |(lo, hi), &(a, b)| (lo.min(a), hi.max(b)))
}

#[allow(clippy::too_many_arguments)]
fn draw_channel_lane(
    p: &egui::Painter,
    area: Rect,
    clip: Rect,
    body_min_x: f32,
    dur_px: f32,
    it: &TrackItem,
    bins: &[(f32, f32)],
    gain: f32,
    dynamic: bool,
    style: WaveformStyle,
    col: Color32,
) {
    let sr = 48_000.0;
    let spp = 256.0;
    let mut mesh = egui::Mesh::default();
    let mut border_pts: Vec<Pos2> = Vec::new();
    let fill_col = if style == WaveformStyle::Bordered { Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 150) } else { col };
    let mid_y = area.center().y;
    let half_h = area.height() * 0.5;

    for x in (clip.min.x.floor() as i32)..(clip.max.x.ceil() as i32) {
        let xf = x as f32;
        let f0 = (xf - body_min_x) / dur_px;
        let t0 = it.start + Tick((it.duration.0 as f64 * f0 as f64) as i64);
        let (s0, s1) = bucket_range_for_pixel(it, x, body_min_x, dur_px, sr, spp);
        let (lo, hi) = sample_channel_minmax(bins, s0, s1);
        let vol = crate::panels::timeline_volume::gain_at(it, t0) * gain;
        match style {
            WaveformStyle::Rectified | WaveformStyle::Bordered => {
                let m = lo.abs().max(hi.abs()) * vol;
                let h = scale_amplitude(m, dynamic) * area.height();
                let y_top = area.max.y - h;
                if h > 0.3 {
                    mesh.add_colored_rect(Rect::from_min_max(pos2(xf, y_top), pos2(xf + 1.0, area.max.y)), fill_col);
                }
                if style == WaveformStyle::Bordered {
                    border_pts.push(pos2(xf + 0.5, y_top.min(area.max.y - 0.5)));
                }
            }
            WaveformStyle::Centered | WaveformStyle::StereoSplit => {
                let up = scale_amplitude(hi.max(0.0) * vol, dynamic) * half_h;
                let dn = scale_amplitude(lo.min(0.0).abs() * vol, dynamic) * half_h;
                if up + dn > 0.3 {
                    mesh.add_colored_rect(Rect::from_min_max(pos2(xf, mid_y - up), pos2(xf + 1.0, mid_y + dn)), fill_col);
                }
            }
        }
    }

    p.add(mesh);
    if style == WaveformStyle::Bordered && border_pts.len() >= 2 {
        let outline_col = Color32::from_rgb(col.r().saturating_add(55), col.g().saturating_add(55), col.b().saturating_add(55));
        p.add(egui::Shape::line(border_pts, Stroke::new(1.15, outline_col)));
    }
    if matches!(style, WaveformStyle::Centered | WaveformStyle::StereoSplit) {
        p.line_segment([pos2(clip.min.x, mid_y), pos2(clip.max.x, mid_y)], Stroke::new(0.75, Color32::from_black_alpha(65)));
    }
}

fn draw_channel_badge(p: &egui::Painter, rect: Rect, label: &str) {
    p.rect_filled(rect, 1.0, Color32::from_black_alpha(160));
    p.text(rect.center(), Align2::CENTER_CENTER, label, Tokens::ui(7.5), Color32::from_rgb(0xd9, 0xd9, 0xd9));
}

pub(super) fn draw_waveform(app: &mut FilmcraftApp, ctx: &egui::Context, p: &egui::Painter, body: Rect, it: &TrackItem, col: Color32) {
    let zone_h = waveform_zone_height(body.height(), app.ui.timeline.waveform_full_height);
    let area = Rect::from_min_max(pos2(body.min.x + 1.0, body.max.y - zone_h - 1.0), pos2(body.max.x - 1.0, body.max.y - 1.0));
    let clip = p.clip_rect().intersect(area);
    if clip.width() <= 0.0 || clip.height() <= 0.0 {
        return;
    }

    let Some(peaks) = request_peaks(app, ctx, it.item, it.audio_stream) else {
        // Pending background generation: draw a subtle dashed baseline so the user knows it's loading.
        let y = area.center().y;
        p.add(egui::Shape::dashed_line(
            &[pos2(clip.min.x, y), pos2(clip.max.x, y)],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 95)),
            4.0,
            4.0,
        ));
        return;
    };

    let pkey = (it.item, it.audio_stream);
    let key = Arc::as_ptr(&peaks) as usize;
    let peak = match app.tl.peak_max.get(&pkey) {
        Some(&(k, v)) if k == key => v,
        _ => {
            app.tl.peak_max.insert(pkey, (key, peaks.max_peak));
            peaks.max_peak
        }
    };
    let gain = waveform_display_gain(peak, it.gain_db);
    let dynamic = app.ui.extras.dynamic_waveforms;
    let style = app.ui.timeline.waveform_style;
    let dur_px = body.width().max(1.0);

    if style == WaveformStyle::StereoSplit && peaks.channels.len() >= 2 {
        let mid_y = area.center().y;
        let top_area = Rect::from_min_max(area.min, pos2(area.max.x, (mid_y - 1.0).max(area.min.y + 1.0)));
        let bot_area = Rect::from_min_max(pos2(area.min.x, (mid_y + 1.0).min(area.max.y - 1.0)), area.max);
        let top_clip = clip.intersect(top_area);
        let bot_clip = clip.intersect(bot_area);
        if top_clip.width() > 0.0 && top_clip.height() > 0.0 {
            draw_channel_lane(p, top_area, top_clip, body.min.x, dur_px, it, &peaks.channels[0], gain, dynamic, style, col);
        }
        if bot_clip.width() > 0.0 && bot_clip.height() > 0.0 {
            draw_channel_lane(p, bot_area, bot_clip, body.min.x, dur_px, it, &peaks.channels[1], gain, dynamic, style, col);
        }
        p.line_segment([pos2(clip.min.x, mid_y), pos2(clip.max.x, mid_y)], Stroke::new(0.8, Color32::from_black_alpha(120)));
        if body.width() > 30.0 {
            if top_area.height() >= 11.0 {
                draw_channel_badge(p, Rect::from_min_size(pos2(body.min.x + 3.0, top_area.max.y - 10.0), vec2(8.5, 9.0)), "L");
            }
            if bot_area.height() >= 11.0 {
                draw_channel_badge(p, Rect::from_min_size(pos2(body.min.x + 3.0, bot_area.max.y - 10.0), vec2(8.5, 9.0)), "R");
            }
        }
    } else {
        draw_channel_lane(p, area, clip, body.min.x, dur_px, it, &peaks.combined, gain, dynamic, style, col);
        if body.width() > 30.0 {
            let label = if peaks.channels.len() >= 2 { "2" } else { "1" };
            draw_channel_badge(p, Rect::from_min_size(pos2(body.min.x + 3.0, body.max.y - 11.0), vec2(8.0, 9.0)), label);
        }
    }
}

/// Display gain for a clip's waveform: clip gain, plus normalisation of the source to full scale
/// so quiet recordings stay readable, but by at most +12 dB, so a near-silent track (room tone at
/// −50 dBFS, a silent film's empty audio stream) does not look like a loud one.
pub(super) fn waveform_display_gain(source_peak: f32, clip_gain_db: f64) -> f32 {
    const MAX_BOOST: f32 = 3.981_072; // +12 dB
    let boost = (1.0 / source_peak.max(1e-9)).min(MAX_BOOST);
    filmcraft_render::audio::db_to_gain(clip_gain_db) * boost
}

/// A key that changes when the sound of sequence `item` does: its audio segments (clips,
/// transitions, mixer state; see `filmcraft_render::preview::audio_segments`) and its length.
fn sequence_audio_key(project: &filmcraft_project::Project, item: ItemId) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for seg in filmcraft_render::preview::audio_segments(project, item) {
        (seg.first_sample, seg.samples, seg.hash).hash(&mut h);
    }
    project.sequence(item).map(|q| (q.duration().0, q.settings.sample_rate)).hash(&mut h);
    h.finish()
}

/// Waveform peaks of a nested sequence's mix, in the same form as a media item's (min, max per
/// 256 samples at 48 kHz). They are mixed in the background, and again whenever the sequence's
/// sound changes; until the new ones are ready the old ones stay on show.
fn request_sequence_peaks(app: &mut FilmcraftApp, ctx: &egui::Context, item: ItemId) -> Option<Arc<WaveformPeaks>> {
    let revision = app.session.revision;
    let key = match app.tl.seq_keys_seen.get(&item) {
        Some(&(rev, key)) if rev == revision => key,
        _ => {
            let key = sequence_audio_key(&app.session.project, item);
            app.tl.seq_keys_seen.insert(item, (revision, key));
            key
        }
    };
    let pkey = (item, 0);
    let have = app.tl.peaks.lock().unwrap_or_else(|e| e.into_inner()).get(&pkey).cloned();
    let current = app.tl.seq_peak_keys.lock().unwrap_or_else(|e| e.into_inner()).get(&item) == Some(&key);
    if current && have.is_some() {
        return have;
    }
    {
        let mut pend = app.tl.peaks_pending.lock().unwrap_or_else(|e| e.into_inner());
        if pend.contains(&pkey) {
            return have;
        }
        pend.push(pkey);
    }
    let (peaks, keys, pending, peaks_gen) = (app.tl.peaks.clone(), app.tl.seq_peak_keys.clone(), app.tl.peaks_pending.clone(), app.tl.peaks_gen.clone());
    let gen_at_start = peaks_gen.load(Ordering::Acquire);
    let project = app.session.project.clone();
    let provider = app.session.media.provider(project.clone(), app.session.services.clone());
    let ctx = ctx.clone();
    let run = move || {
        // a panic in the mix must not take the app down with it, nor leave the item pending for good
        let mixed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let seq = project.sequence(item)?;
            let sr = seq.settings.sample_rate.max(1);
            let total = seq.duration().to_units_floor(sr as i64).max(0) as usize;
            // one peak per 256 samples at 48 kHz, whatever the sequence's own rate
            let bucket = ((256u64 * sr as u64) / 48_000).max(1) as usize;
            let chunk = (bucket * 750).max(1);
            let cap = total / bucket + 1;
            let mut ch0 = Vec::with_capacity(cap);
            let mut ch1 = Vec::with_capacity(cap);
            let mut has_stereo = false;
            let mut s = 0usize;
            while s < total {
                let n = chunk.min(total - s);
                let buf = filmcraft_render::audio::mix_sequence(&project, seq, s as i64, n, &provider);
                let left = buf.channels.first()?;
                let right = buf.channels.get(1);
                if right.is_some() {
                    has_stereo = true;
                }
                for (ci, c) in left.chunks(bucket).enumerate() {
                    let (lo, hi) = c.iter().fold((0f32, 0f32), |(l, h), v| (l.min(*v), h.max(*v)));
                    ch0.push((lo, hi));
                    if let Some(rc) = right.and_then(|r| r.chunks(bucket).nth(ci)) {
                        let (rlo, rhi) = rc.iter().fold((0f32, 0f32), |(l, h), v| (l.min(*v), h.max(*v)));
                        ch1.push((rlo, rhi));
                    }
                }
                s += n;
            }
            let channels = if has_stereo && ch1.len() == ch0.len() { vec![ch0, ch1] } else { vec![ch0] };
            Some(WaveformPeaks::from_channels(channels))
        }));
        if let Ok(Some(out)) = mixed
            && peaks_gen.load(Ordering::Acquire) == gen_at_start
        {
            peaks.lock().unwrap_or_else(|e| e.into_inner()).insert(pkey, Arc::new(out));
            keys.lock().unwrap_or_else(|e| e.into_inner()).insert(item, key);
        }
        pending.lock().unwrap_or_else(|e| e.into_inner()).retain(|i| *i != pkey);
        ctx.request_repaint();
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(run);
    #[cfg(target_arch = "wasm32")]
    run();
    have
}

/// Min/max peaks (one per 256 samples at 48 kHz) of audio stream `stream` of `src`, `dur` long.
///
/// Decodes all channels (preserving L/R for stereo display, and folding extra channels into the
/// combined envelope). If a chunk fails to decode, retries once before zero-filling that chunk and
/// continuing so the rest of the clip is not truncated. Returns `None` if every chunk failed so
/// transient decode errors are not permanently cached as silence.
pub(super) fn source_peaks(src: &dyn filmcraft_media::MediaSource, stream: usize, dur: Tick) -> Option<WaveformPeaks> {
    let sr = 48_000u32;
    let total = dur.to_units_floor(sr as i64).max(0) as usize;
    let cap = total / 256 + 1;
    let mut channels: Vec<Vec<(f32, f32)>> = Vec::new();
    let chunk = 48_000usize;
    let mut s = 0usize;
    let mut any_ok = false;

    while s < total {
        let n = chunk.min(total - s);
        let decoded = src.audio_stream(stream, s as i64, n, sr).or_else(|_| src.audio_stream(stream, s as i64, n, sr));
        match decoded {
            Ok(buf) => {
                any_ok = true;
                let ch_count = buf.channels.len().max(1);
                if channels.is_empty() {
                    channels = (0..ch_count).map(|_| Vec::with_capacity(cap)).collect();
                } else if channels.len() < ch_count {
                    let existing = channels.first().map_or(0, Vec::len);
                    while channels.len() < ch_count {
                        channels.push(vec![(0.0, 0.0); existing]);
                    }
                }
                let expected_buckets = n.div_ceil(256);
                for (ci, out_ch) in channels.iter_mut().enumerate() {
                    if let Some(samples) = buf.channels.get(ci) {
                        let mut pushed = 0usize;
                        for c in samples.chunks(256) {
                            let (lo, hi) = c.iter().fold((0f32, 0f32), |(l, h), v| (l.min(*v), h.max(*v)));
                            out_ch.push((lo, hi));
                            pushed += 1;
                        }
                        while pushed < expected_buckets {
                            out_ch.push((0.0, 0.0));
                            pushed += 1;
                        }
                    } else {
                        out_ch.resize(out_ch.len() + expected_buckets, (0.0, 0.0));
                    }
                }
            }
            Err(_) => {
                let expected_buckets = n.div_ceil(256);
                if channels.is_empty() {
                    channels.push(Vec::with_capacity(cap));
                }
                for out_ch in &mut channels {
                    out_ch.resize(out_ch.len() + expected_buckets, (0.0, 0.0));
                }
            }
        }
        s += n;
        std::thread::yield_now();
    }

    if total > 0 && !any_ok {
        return None;
    }
    if channels.is_empty() {
        channels.push(Vec::new());
    }
    Some(WaveformPeaks::from_channels(channels))
}

/// Waveform peaks of audio stream `stream` of `item` (a clip's `audio_stream`).
pub(crate) fn request_peaks(app: &mut FilmcraftApp, ctx: &egui::Context, item: ItemId, stream: usize) -> Option<Arc<WaveformPeaks>> {
    if app.session.project.sequence(item).is_some() {
        return request_sequence_peaks(app, ctx, item);
    }
    let pkey = (item, stream);
    if let Some(p) = app.tl.peaks.lock().unwrap_or_else(|e| e.into_inner()).get(&pkey) {
        return Some(p.clone());
    }
    // No source: try again on a later frame instead of leaving the item pending forever.
    let src = app.session.source(item)?;
    {
        let mut pend = app.tl.peaks_pending.lock().unwrap_or_else(|e| e.into_inner());
        if pend.contains(&pkey) {
            return None;
        }
        pend.push(pkey);
    }
    let peaks = app.tl.peaks.clone();
    let pending = app.tl.peaks_pending.clone();
    let peaks_gen = app.tl.peaks_gen.clone();
    let gen_at_start = peaks_gen.load(Ordering::Acquire);
    let dur = src.info().duration;
    let ctx = ctx.clone();
    let run = move || {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source_peaks(&*src, stream, dur)));
        if let Ok(Some(out)) = res
            && peaks_gen.load(Ordering::Acquire) == gen_at_start
        {
            peaks.lock().unwrap_or_else(|e| e.into_inner()).insert(pkey, Arc::new(out));
        }
        pending.lock().unwrap_or_else(|e| e.into_inner()).retain(|i| *i != pkey);
        ctx.request_repaint();
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(run);
    #[cfg(target_arch = "wasm32")]
    run();
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use filmcraft_frame::{AudioBuffer, VideoFrame};
    use filmcraft_media::{FrameRequest, MediaError, MediaInfo, MediaKind, MediaSource};
    use filmcraft_time::Tick;

    use super::{WaveformPeaks, bucket_range_for_pixel, source_peaks};

    struct MockStereoSource {
        info: MediaInfo,
        fail_first_chunk: bool,
    }

    impl MockStereoSource {
        fn new(dur: Tick, fail_first_chunk: bool) -> Self {
            Self {
                info: MediaInfo {
                    name: "stereo.wav".into(),
                    kind: MediaKind::Movie,
                    duration: dur,
                    video: None,
                    audio_streams: Vec::new(),
                    container: "wav".into(),
                    start_timecode: None,
                    file_size: None,
                },
                fail_first_chunk,
            }
        }
    }

    impl MediaSource for MockStereoSource {
        fn info(&self) -> &MediaInfo {
            &self.info
        }

        fn video_frame(&self, _req: FrameRequest) -> filmcraft_media::Result<Arc<VideoFrame>> {
            Err(MediaError::NoStream("video"))
        }

        fn audio(&self, start: i64, frames: usize, sr: u32) -> filmcraft_media::Result<AudioBuffer> {
            self.audio_stream(0, start, frames, sr)
        }

        fn audio_stream(&self, _stream: usize, start: i64, frames: usize, sr: u32) -> filmcraft_media::Result<AudioBuffer> {
            if self.fail_first_chunk && start == 0 {
                return Err(MediaError::Unsupported("transient first-chunk failure".into()));
            }
            // Left channel silent, Right channel has 0.75 pulse so ch0-only would be flat!
            let ch0 = vec![0.0f32; frames];
            let ch1 = vec![0.75f32; frames];
            Ok(AudioBuffer { sample_rate: sr, channels: vec![ch0, ch1] })
        }
    }

    #[test]
    fn stereo_source_with_audio_only_on_right_channel_is_not_flat() {
        let src = MockStereoSource::new(Tick::from_seconds_f64(2.0), false);
        let peaks = source_peaks(&src, 0, Tick::from_seconds_f64(2.0)).expect("peaks should succeed");
        assert_eq!(peaks.channels.len(), 2);
        assert!(peaks.max_peak > 0.7);
        assert!(peaks.combined.iter().any(|&(_, hi)| hi > 0.7));
        assert!(peaks.channels[0].iter().all(|&(lo, hi)| lo == 0.0 && hi == 0.0));
        assert!(peaks.channels[1].iter().any(|&(_, hi)| hi > 0.7));
    }

    #[test]
    fn intermediate_chunk_error_does_not_truncate_remaining_waveform() {
        let src = MockStereoSource::new(Tick::from_seconds_f64(2.5), true);
        let peaks = source_peaks(&src, 0, Tick::from_seconds_f64(2.5)).expect("later chunks should still decode");
        // First second failed (188 buckets), remaining 1.5s should still be present with non-zero peaks.
        assert!(peaks.combined.len() > 350);
        assert!(peaks.combined.last().is_some_and(|&(_, hi)| hi > 0.7));
    }

    #[test]
    fn reverse_clip_bucket_range_is_ordered_and_non_empty() {
        let mut session = filmcraft_engine::Session::default();
        session.execute("file.openDemoProject", serde_json::json!({})).unwrap();
        let mut it = session.active_sequence().expect("demo sequence").audio_tracks[0].items[0].clone();
        it.reverse = true;
        let (lo, hi) = bucket_range_for_pixel(&it, 50, 0.0, 200.0, 48_000.0, 256.0);
        assert!(hi > lo);
    }

    #[test]
    fn waveform_peaks_derefs_to_combined_envelope() {
        let wp = WaveformPeaks::from_channels(vec![vec![(-0.1, 0.2)], vec![(-0.6, 0.8)]]);
        assert_eq!(wp.len(), 1);
        assert_eq!(wp[0], (-0.6, 0.8));
        assert!((wp.max_peak - 0.8).abs() < 1e-6);
    }
}
