use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use ssx_content::check::{Checked, Severity};
use ssx_content::manifest::MANIFEST_FILE;
use ssx_content::movie::card::Card;

use super::check::push;
use crate::content::fmv::ffmpeg::{AUDIO_ARGS, Encode, VIDEO_ARGS, encode};

pub(super) fn check(c: &mut Checked, root: &Path) {
    let movies = std::mem::take(&mut c.movies.movies);
    for (id, (file, walked)) in &movies {
        decode(c, root, id, file, walked.info.duration_seconds());
    }
    c.movies.movies = movies;
}

#[cfg(feature = "movie-decode")]
fn decode(c: &mut Checked, root: &Path, id: &ssx_content::ContentId, file: &str, seconds: f64) {
    use ssx_content::movie::Limits;
    let path = root.to_path_buf();
    let rel = file.to_string();
    let open = move || -> Result<(std::fs::File, u64), String> {
        let tree = ssx_content::path::PackageTree::open(&path).map_err(|e| e.to_string())?;
        tree.open_read(&rel, Limits::default().max_file_bytes).map_err(|e| e.to_string())
    };
    let threads = ssx_movie::Threads { threads: 0, frame_delay: 0 };
    let budget = ssx_movie::check::budget(seconds);
    match ssx_movie::check::full_decode(open, &Limits::default(), threads, budget) {
        Ok(r) => {
            let message = format!(
                "{id}: decoded to its end: {} pictures, {} sound frames in {:.2} s ({:.1} pictures a second, {:.2}× \
                 real time; a picture's time p50 {:.2} ms, p99 {:.2} ms)",
                r.pictures,
                r.audio_frames,
                r.seconds,
                r.fps(),
                r.speed(),
                r.frame_ms_quantile(0.5),
                r.frame_ms_quantile(0.99)
            );
            push(c, Severity::Note, file, (0, 0), message);
            if r.speed() < 1.0 {
                let message = format!(
                    "{id}: decodes slower than real time on this machine ({:.2}×): the player drops pictures to keep \
                     the sound's time (a smaller picture or fewer pictures a second decode faster)",
                    r.speed()
                );
                push(c, Severity::Warning, file, (0, 0), message);
            }
        }
        Err(e) => push(c, Severity::Error, MANIFEST_FILE, (0, 0), format!("{id}: {file}: {e}")),
    }
}

#[cfg(not(feature = "movie-decode"))]
fn decode(c: &mut Checked, _root: &Path, id: &ssx_content::ContentId, file: &str, _seconds: f64) {
    let message = format!("{id}: read, not decoded: this program is built without the movie decoders");
    push(c, Severity::Note, file, (0, 0), message);
}

const README: &str = "This package replaces the game's boot movie (ssx:movie/boot_movie_eng) with a movie of its own.

A movie is WebM: AV1 video with Vorbis sound (`.webm`). This one was made by `ssx-assets mod new --template movie`
from the template's own pictures and tone, with FFmpeg:

  ffmpeg -f rawvideo -pix_fmt rgb24 -s 1280x720 -r 30/1 -i pictures.rgb -f f32le -ar 48000 -ac 2 -ch_layout stereo \\
    -i sound.f32 -map 0:v -map 1:a {AUDIO} {VIDEO} -color_range tv -f webm movies/intro.webm

For your own movie (any source FFmpeg reads):

  ffmpeg -i my_movie.mov {VIDEO} -color_range tv {AUDIO} -ac 2 movies/intro.webm

What the game plays: one AV1 video track (Main profile, 8-bit 4:2:0, one operating point, no film grain) of at most
1920 x 1080 and 60 pictures a second, at most one Vorbis sound track (1, 2 or 6 channels; 8 000 to 192 000 Hz), at
most 30 minutes and 512 MiB. `ssx-assets mod check` reads and decodes it to its end and tells how fast it decodes on
your machine; slower than real time, the game drops pictures to keep the sound's time.

The port's movie slot `ava:movie/fe_background` is empty by default: a movie your package gives it with
`[override]` plays on the front end behind the globe, in place of the game's backdrop picture, while the globe
shows. It covers the screen at its own aspect (centred; what lies past the edges is left out), loops and plays
without its sound (a sound track is allowed and left silent).

Ids: `ssx-assets content ids --kind movie` lists the game's movies; `ssx-assets content export ssx:movie/<name>`
writes one from your own disc as a WebM, for your own use only: exported files may not be shipped (`mod check` and
`mod pack` refuse them). A package's movie must be its author's own work, or material whose licence lets them share it.
";

pub fn template(root: &Path, name: &str) -> Result<()> {
    std::fs::create_dir(root.join("movies"))?;
    let card = Card { width: 1280, height: 720, fps: (30, 1), seconds: 6, rate: 48_000, channels: 2 };
    let sound = root.join("movies").join(".template-sound.f32");
    let written = (|| -> std::io::Result<()> {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&sound)?);
        for x in card.audio(0, u64::from(card.seconds) * u64::from(card.rate)) {
            f.write_all(&x.to_le_bytes())?;
        }
        f.flush()
    })();
    if let Err(e) = written {
        std::fs::remove_file(&sound).ok();
        return Err(anyhow::Error::new(e).context("the template's sound"));
    }
    let scene = card.scene();
    let pictures = card.pictures();
    let enc = Encode {
        width: card.width,
        height: card.height,
        fps: card.fps,
        pix_fmt: "rgb24",
        audio: Some((&sound, card.rate, card.channels, "stereo")),
        timeout: Duration::from_secs(600),
    };
    let made = encode(&root.join("movies").join("intro.webm"), &enc, move |k, buf| {
        if k >= pictures {
            return false;
        }
        buf.resize((card.width * card.height * 3) as usize, 0);
        scene.picture(k, buf);
        true
    });
    std::fs::remove_file(&sound).ok();
    made.context("the template's movie")?;
    let body = r#"
# A movie of your own in place of one of the game's (`ssx-assets content ids --kind movie` lists them): WebM, AV1 video
# with Vorbis sound (README.txt: how to make one with FFmpeg, and what the game plays).
[override]
"ssx:movie/boot_movie_eng" = "movies/intro.webm"
# The port's movie slot behind the globe (empty by default): a movie in place of the front end's backdrop picture
# while the globe shows, covering the screen at its own aspect, looping, without its sound.
# "ava:movie/fe_background" = "movies/intro.webm"

# A movie the package adds ("<package>:movie/<path>"): played by its id where the game takes one (the test option
# `avalanche-game --play-movie <id>`; scripts and maps name movies from phases M7 and M8).
# [add]
# "movie/intro" = "movies/intro.webm"
"#;
    let manifest = format!(
        "# The manifest of the package \"{name}\" (Project Avalanche; every key: Docs/architecture/content.md §2, §20).\n\
         # Check it with `ssx-assets mod check <folder>`, pack it with `ssx-assets mod pack <folder>`.\n\n\
         [resource]\nname = \"{name}\"\nversion = \"0.1.0\"\ntitle = \"{name}\"\nauthors = []\ndescription = \"\"\n\
         api = 1\ntype = \"mod\"\npriority = 0\n{body}"
    );
    std::fs::write(root.join(MANIFEST_FILE), manifest)?;
    let readme = README.replace("{VIDEO}", &VIDEO_ARGS.join(" ")).replace("{AUDIO}", &AUDIO_ARGS.join(" "));
    std::fs::write(root.join("README.txt"), readme)?;
    Ok(())
}
