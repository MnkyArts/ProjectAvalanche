use std::io::Cursor;

use super::write::*;
use super::*;

const SEQ: Seq = BASE_SEQ;

fn tracks(video: Vec<u8>, audio: Option<Vec<u8>>) -> Vec<u8> {
    element(0x1654_AE6B, &[video, audio.unwrap_or_default()].concat())
}

fn good_tracks() -> Vec<u8> {
    let setup = vorbis_setup(&[(16, 2, 1)]);
    tracks(
        video_track(1, 320, 180, &av1_private(SEQ), Some(33_333_333)),
        Some(audio_track(2, 2, 48_000.0, &vorbis_private(2, 48_000, &setup))),
    )
}

fn movie(tracks: Vec<u8>, blocks: Vec<Vec<u8>>) -> Vec<u8> {
    file(&[info(1000.0), tracks, cluster(0, &blocks)])
}

fn good_blocks() -> Vec<Vec<u8>> {
    vec![simple_block(1, 0, 0x80, &unit()), simple_block(2, 0, 0x80, &[0, 1, 2]), simple_block(1, 34, 0, &unit())]
}

fn walked(b: &[u8]) -> Result<Walked> {
    walk(Cursor::new(b), b.len() as u64, &Limits::default())
}

fn refused(b: &[u8]) -> String {
    match walked(b) {
        Ok(w) => panic!("accepted: {} pictures", w.pictures),
        Err(e) => e.message,
    }
}

#[test]
fn a_movie_of_our_writer_walks() {
    let w = walked(&movie(good_tracks(), good_blocks())).unwrap();
    assert_eq!((w.pictures, w.keyframes, w.audio_packets), (2, 1, 1));
    assert_eq!((w.info.video.width, w.info.video.height, w.info.duration_ns), (320, 180, 1_000_000_000));
    assert_eq!(w.last_picture_ns, 34_000_000);
    let a = w.info.audio.unwrap();
    assert_eq!((a.channels, a.rate, a.headers.codebooks), (2, 48_000, 1));
}

#[test]
fn anything_but_webm_is_refused_by_what_it_is() {
    let mut vp6 = b"MVhd".to_vec();
    vp6.extend([0u8; 60]);
    assert!(refused(&vp6).contains("an EA movie"));
    assert!(refused(b"OggS\0\0\0\0").contains("OGG"));
    let mut mkv = ebml_header("matroska");
    mkv.extend(element(0x1853_8067, &[info(1000.0), good_tracks()].concat()));
    assert!(refused(&mkv).contains("\"matroska\" document"));
}

#[test]
fn sizes_reaching_past_their_parent_or_the_file_are_refused() {
    let m = movie(good_tracks(), good_blocks());
    for cut in [10, 40, m.len() / 2, m.len() - 1] {
        let e = refused(&m[..cut]);
        assert!(e.contains("past") || e.contains("cut short") || e.contains("ends"), "{cut}: {e}");
    }
    let mut unknown = ebml_header("webm");
    unknown.extend(element_with(0x1853_8067, &[info(1000.0), good_tracks()].concat(), None, Some(u64::MAX)));
    assert!(refused(&unknown).contains("states no size"));
    let mut tail = m.clone();
    tail.extend([0x42, 0x42, 0x42]);
    assert!(refused(&tail).contains("past its parent") || refused(&tail).contains("after the Segment"));
}

#[test]
fn the_headers_hold_the_caps() {
    let hours = file(&[info(5.0 * 3600.0 * 1000.0), good_tracks(), cluster(0, &good_blocks())]);
    assert!(refused(&hours).contains("a duration of 18000.000 s"));
    let big = tracks(video_track(1, 16_384, 16_384, &av1_private(SEQ), None), None);
    assert!(refused(&movie(big, good_blocks())).contains("a width of 16384 pixels"));
    let fast = tracks(video_track(1, 320, 180, &av1_private(SEQ), Some(100_000)), None);
    assert!(refused(&movie(fast, good_blocks())).contains("10000 pictures a second"));
    let two = tracks(
        [video_track(1, 320, 180, &av1_private(SEQ), None), video_track(3, 320, 180, &av1_private(SEQ), None)].concat(),
        None,
    );
    assert!(refused(&movie(two, good_blocks())).contains("a second video track"));
    let vp9 = element(0x1654_AE6B, &element(0xAE, &[uint(0xD7, 1), uint(0x83, 1), string(0x86, "V_VP9")].concat()));
    assert!(refused(&movie(vp9, good_blocks())).contains("\"V_VP9\""));
    let subs = [
        tracks(video_track(1, 320, 180, &av1_private(SEQ), None), None),
        element(0xAE, &[uint(0xD7, 4), uint(0x83, 17)].concat()),
    ];
    assert!(
        refused(&file(&[info(1000.0), element(0x1654_AE6B, &subs.concat()[..]), cluster(0, &good_blocks())]))
            .contains("Tracks")
    );
}

#[test]
fn blocks_hold_their_caps() {
    let lace = |frames: usize, sizes: &[u8], data: usize| {
        let mut b = vec![u8::try_from(frames - 1).unwrap()];
        b.extend_from_slice(sizes);
        b.extend(vec![7u8; data]);
        b
    };
    let bomb = simple_block(2, 0, 0x80 | 0x02, &lace(256, &[1; 255], 256));
    let e = refused(&movie(good_tracks(), vec![simple_block(1, 0, 0x80, &unit()), bomb]));
    assert!(e.contains("256 frames laced in one block (at most 64)"), "{e}");
    let past = simple_block(2, 0, 0x82, &lace(3, &[255, 255, 9, 1], 20));
    assert!(refused(&movie(good_tracks(), vec![simple_block(1, 0, 0x80, &unit()), past])).contains("past its block"));
    let empty = simple_block(2, 0, 0x82, &lace(2, &[0], 5));
    assert!(refused(&movie(good_tracks(), vec![simple_block(1, 0, 0x80, &unit()), empty])).contains("an empty frame"));
    let laced_video = simple_block(1, 0, 0x82, &lace(2, &[4], 8));
    assert!(refused(&movie(good_tracks(), vec![laced_video])).contains("laced video"));
    let fast = vec![simple_block(1, 0, 0x80, &unit()), simple_block(1, 1, 0, &unit())];
    assert!(refused(&movie(good_tracks(), fast)).contains("at most 60 a second"));
    let back = vec![simple_block(1, 40, 0x80, &unit()), simple_block(1, 0, 0, &unit())];
    assert!(refused(&movie(good_tracks(), back)).contains("pictures come in order"));
    assert!(refused(&movie(good_tracks(), vec![simple_block(1, 0, 0, &unit())])).contains("not a key frame"));
    assert!(refused(&movie(good_tracks(), vec![simple_block(9, 0, 0x80, &unit())])).contains("track 9"));
    assert!(refused(&movie(good_tracks(), vec![simple_block(1, 0, 0x88, &unit())])).contains("invisible"));
    let late = vec![simple_block(1, 0, 0x80, &unit()), simple_block(1, 2_100, 0, &unit())];
    assert!(refused(&movie(good_tracks(), late)).contains("outside the movie's 1.000 s"));
    let mut garbage = good_blocks();
    garbage.push(vec![0x8F, 0x81, 0x00]);
    assert!(refused(&movie(good_tracks(), garbage)).contains("which the reader does not know"));
}

#[test]
fn temporal_units_are_walked() {
    let other_seq = obu(av1::OBU_SEQUENCE_HEADER, &seq_header(Seq { w: 160, ..SEQ }));
    let swap = [other_seq, unit()].concat();
    assert!(refused(&movie(good_tracks(), vec![simple_block(1, 0, 0x80, &swap)])).contains("other than the track's"));
    let tiles = obu(av1::OBU_TILE_LIST, &[0; 8]);
    assert!(refused(&movie(good_tracks(), vec![simple_block(1, 0, 0x80, &tiles)])).contains("tile list"));
    let none = obu(av1::OBU_METADATA, &[1, 2]);
    assert!(refused(&movie(good_tracks(), vec![simple_block(1, 0, 0x80, &none)])).contains("without a frame"));
    let long = vec![0x32, 0x40, 1, 2];
    assert!(refused(&movie(good_tracks(), vec![simple_block(1, 0, 0x80, &long)])).contains("reaches past"));
    let forbidden = vec![0xB2, 0x01, 0x00];
    assert!(refused(&movie(good_tracks(), vec![simple_block(1, 0, 0x80, &forbidden)])).contains("forbidden"));
}

#[test]
fn sequence_headers_out_of_the_caps_are_refused() {
    let limits = Limits::default();
    let check = |s: Seq| av1::Config::parse(&av1_private(s), &limits).err().unwrap_or_default();
    assert!(check(SEQ).is_empty());
    assert!(check(Seq { mono: true, ..SEQ }).is_empty());
    assert!(check(Seq { profile: 1, ..SEQ }).contains("profile 1"));
    assert!(check(Seq { ten_bit: true, ..SEQ }).contains("10-bit"));
    assert!(check(Seq { points: 2, ..SEQ }).contains("2 operating points"));
    assert!(check(Seq { still: true, ..SEQ }).contains("still picture"));
    assert!(check(Seq { grain: true, ..SEQ }).contains("film grain"));
    assert!(check(Seq { timing_fps: Some(120), ..SEQ }).contains("120 pictures a second"));
    assert!(check(Seq { timing_fps: Some(30), ..SEQ }).is_empty());
    assert!(check(Seq { w: 4096, h: 2160, ..SEQ }).contains("4096 × 2160"));
    assert!(check(Seq { matrix: Some(8), ..SEQ }).contains("matrix coefficients 8"));
    let other = tracks(video_track(1, 320, 176, &av1_private(SEQ), None), None);
    assert!(refused(&movie(other, good_blocks())).contains("its AV1 sequence header 320 × 180"));
}

#[test]
fn vorbis_header_bombs_are_refused_before_a_decoder_is_built() {
    let limits = Limits::default();
    let headers =
        |books: &[(u64, u64, u64)]| vorbis::Headers::parse(&vorbis_private(2, 48_000, &vorbis_setup(books)), &limits);
    let one = headers(&[(16, 2, 1)]).unwrap();
    assert_eq!(one.table_bytes, 16 * 16 + 16 * 2 * 4);
    let bomb = [(131_072, 32, 1); 3];
    let setup = vorbis_setup(&bomb);
    assert!(setup.len() < 128, "{}", setup.len());
    let e = headers(&bomb).unwrap_err();
    assert!(e.contains("a header bomb"), "{e}");
    assert!(headers(&[(131_073, 1, 0)]).unwrap_err().contains("131073 entries"));
    let mut s = vorbis_setup(&[(16, 2, 1)]);
    s.truncate(s.len() - 3);
    assert!(vorbis::Headers::parse(&vorbis_private(2, 48_000, &s), &limits).unwrap_err().contains("ends early"));
}

#[test]
fn laced_frames_come_one_by_one() {
    let frames = |b: &[u8]| -> Vec<usize> {
        let mut d = Demuxer::open(Cursor::new(b), b.len() as u64, &Limits::default()).unwrap();
        let mut out = Vec::new();
        while let Some(p) = d.next_packet().unwrap() {
            if p.track == Track::Audio {
                out.push(p.data.len());
            }
        }
        out
    };
    let video = simple_block(1, 0, 0x80, &unit());
    let mut xiph = vec![2u8, 255, 45, 2];
    xiph.extend(vec![1u8; 307]);
    assert_eq!(frames(&movie(good_tracks(), vec![video.clone(), simple_block(2, 0, 0x82, &xiph)])), [300, 2, 5]);
    let mut ebml = vec![2u8, 0x8A, 0xBC];
    ebml.extend(vec![1u8; 21]);
    assert_eq!(frames(&movie(good_tracks(), vec![video.clone(), simple_block(2, 0, 0x86, &ebml)])), [10, 7, 4]);
    let mut fixed = vec![3u8];
    fixed.extend(vec![1u8; 12]);
    assert_eq!(frames(&movie(good_tracks(), vec![video.clone(), simple_block(2, 0, 0x84, &fixed)])), [3, 3, 3, 3]);
    let mut odd = vec![3u8];
    odd.extend(vec![1u8; 13]);
    assert!(refused(&movie(good_tracks(), vec![video.clone(), simple_block(2, 0, 0x84, &odd)])).contains("fixed lace"));
    let mut group = element(0xA1, &[&[0x81, 0, 0, 0][..], &unit()].concat());
    group.extend(uint(0x75A2, 1_000));
    let g = element(0xA0, &group);
    assert!(refused(&movie(good_tracks(), vec![g])).contains("discard padding"));
}

#[test]
fn a_restart_reads_the_same_packets_again() {
    let m = file(&[
        info(1000.0),
        good_tracks(),
        cluster(0, &good_blocks()),
        cluster(500, &[simple_block(1, 0, 0, &unit()), simple_block(2, 0, 0x80, &[3, 4])]),
    ]);
    let mut d = Demuxer::open(Cursor::new(&m[..]), m.len() as u64, &Limits::default()).unwrap();
    let pass = |d: &mut Demuxer<Cursor<&[u8]>>| {
        let mut out = Vec::new();
        while let Some(p) = d.next_packet().unwrap() {
            out.push(p);
        }
        out
    };
    let first = pass(&mut d);
    assert_eq!(first.iter().filter(|p| p.track == Track::Video).count(), 3);
    assert_eq!(d.counts(), (3, 2));
    d.restart().unwrap();
    assert_eq!(d.counts(), (0, 0));
    assert_eq!(pass(&mut d), first);
    d.restart().unwrap();
    let head = d.next_packet().unwrap().unwrap();
    d.restart().unwrap();
    assert_eq!(head, first[0]);
    assert_eq!(pass(&mut d), first, "a restart from the middle of a pass");
}
