//! What the extraction worker does with a request it did not expect.
//!
//! The child is the security boundary: it confines itself and *then* reads the
//! request, so every byte it parses after that point came from the parent —
//! which builds it from a document, an image or a media file. These tests spawn
//! the real binary and hand it hand-written bytes, the way an attacker who
//! reached the protocol would, and assert that each unexpected shape is refused
//! rather than guessed at.
//!
//! The child is started with `--min-level none`: what is under test is the
//! request parser, and a host that cannot reach the default minimum would
//! otherwise refuse before reading anything — turning every assertion below
//! into a test of this machine rather than of the parser.
//!
//! Every case is bounded by a deadline and killed when it expires, and each
//! case asserts the run did *not* time out: a child that hangs is the failure a
//! bare "exited non-zero" check would read as a pass.
//!
//! The protocol constants are spelled here rather than imported. `MAGIC` is
//! `pub(crate)` for the jobs that share it, and a wire format is what this file
//! is about: a test that reached for the parent's copy would follow it wherever
//! it moved instead of pinning what the child actually reads.

use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// The magic that opens every request and the self-test report.
const MAGIC: &[u8] = b"NFX1";

/// Deadline for one child run. The worker's own request timeout is fifteen
/// seconds at its longest, so a run that reaches this is a hang.
const DEADLINE: Duration = Duration::from_secs(30);

/// What one run of the worker produced.
struct Answer {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: String,
    timed_out: bool,
}

impl Answer {
    /// The reply frame, or a panic naming what came back instead.
    fn frame(&self) -> (u8, &[u8]) {
        assert!(
            self.status.success(),
            "the child must answer, not fail: status={:?} stderr={}",
            self.status,
            self.stderr
        );
        let (&tag, rest) = self
            .stdout
            .split_first()
            .unwrap_or_else(|| panic!("no reply: stderr={}", self.stderr));
        assert!(
            rest.len() >= 4,
            "a reply carries a length: {:?}",
            self.stdout
        );
        let length = u32::from_le_bytes(rest[..4].try_into().expect("four bytes")) as usize;
        assert_eq!(
            rest.len(),
            4 + length,
            "the framing carries exactly the body: {:?}",
            self.stdout
        );
        (tag, &rest[4..])
    }

    /// The reason the child gave, which has to be a refusal frame.
    fn refusal(&self) -> &str {
        let (tag, body) = self.frame();
        assert_eq!(
            tag, b'U',
            "an unexpected request is a refusal, not {:?}",
            tag as char
        );
        std::str::from_utf8(body).expect("a refusal is text")
    }

    /// A run that ended by itself, however it ended.
    fn finished(&self, case: &str) {
        assert!(
            !self.timed_out,
            "{case}: the child hung instead of answering"
        );
    }
}

/// Run the worker once with `args` and `request` on its standard input.
fn run_worker(args: &[&str], request: &[u8]) -> Answer {
    let mut child = spawn(args);
    let mut stdin = child.stdin.take().expect("a stdin pipe");
    let request = request.to_vec();
    let writer = std::thread::spawn(move || {
        // The child may refuse and exit before it reads, which closes the pipe.
        // That is the refusal under test, not a failure here.
        let _ = stdin.write_all(&request);
    });

    let deadline = Instant::now() + DEADLINE;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait().expect("the child is waitable") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                timed_out = true;
                let _ = child.kill();
                break child.wait().expect("a killed child is collectable");
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let _ = writer.join();

    let mut stdout = Vec::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_end(&mut stdout);
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    Answer {
        status,
        stdout,
        stderr,
        timed_out,
    }
}

fn spawn(args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_nanofile"))
        .arg("extract-worker")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the worker binary starts")
}

/// A document request: the magic and the plan's tag, then the bytes.
fn document_request(tag: u8, data: &[u8]) -> Vec<u8> {
    let mut request = Vec::from(MAGIC);
    request.push(tag);
    request.extend_from_slice(data);
    request
}

/// An image request: the magic, the operation, a little-endian target size,
/// then the image bytes.
fn image_request(op: u8, size: u32, data: &[u8]) -> Vec<u8> {
    let mut request = Vec::from(MAGIC);
    request.push(op);
    request.extend_from_slice(&size.to_le_bytes());
    request.extend_from_slice(data);
    request
}

/// A media request head: the magic, the kind, a little-endian size, a
/// little-endian path length, then the path.
fn media_request(kind: u8, size: u32, path: &[u8]) -> Vec<u8> {
    let mut request = media_head(kind, size, path.len() as u16);
    request.extend_from_slice(path);
    request
}

fn media_head(kind: u8, size: u32, path_len: u16) -> Vec<u8> {
    let mut request = Vec::from(MAGIC);
    request.push(kind);
    request.extend_from_slice(&size.to_le_bytes());
    request.extend_from_slice(&path_len.to_le_bytes());
    request
}

/// The documents child parses what it is given and nothing else.
#[test]
fn a_document_request_is_parsed_or_refused() {
    let args = ["--profile", "documents", "--min-level", "none"];

    // The positive control: the harness reaches a working child, so the
    // refusals below are the parser's doing and not a binary that never ran.
    let answer = run_worker(&args, &document_request(0, b"zebraquartz"));
    answer.finished("a text request");
    let (tag, body) = answer.frame();
    assert_eq!(tag, b'T');
    assert_eq!(body, b"zebraquartz");

    // An unknown plan tag is refused rather than mapped onto the nearest plan.
    let answer = run_worker(&args, &document_request(9, b"zebraquartz"));
    answer.finished("an unknown plan tag");
    assert!(
        !answer.status.success(),
        "an unknown plan is an error, not a reply"
    );
    assert!(
        answer.stderr.contains("unknown plan tag"),
        "the reason has to name the tag: {}",
        answer.stderr
    );

    // A request that is not this protocol at all.
    let mut wrong_magic = Vec::from(b"XXXX" as &[u8]);
    wrong_magic.push(0);
    let answer = run_worker(&args, &wrong_magic);
    answer.finished("a foreign magic");
    assert!(!answer.status.success(), "a foreign magic is not a request");
    assert!(
        answer.stderr.contains("protocol magic"),
        "the reason has to name the magic: {}",
        answer.stderr
    );

    // Truncated input: the child waits for the prologue, and the close of
    // standard input is what ends that wait.
    let answer = run_worker(&args, MAGIC);
    answer.finished("a truncated prologue");
    assert!(!answer.status.success(), "a truncated request is an error");
}

/// The switch is enforced in the child too, not only in the parent that
/// decides not to spawn it.
#[test]
fn the_child_refuses_to_serve_with_the_sandbox_off() {
    let answer = run_worker(
        &[
            "--profile",
            "documents",
            "--min-level",
            "none",
            "--sandbox-off",
        ],
        &document_request(0, b"zebraquartz"),
    );
    answer.finished("the switch off");
    assert!(!answer.status.success(), "the child must not serve");
    assert!(
        answer.stderr.contains("sandbox unavailable"),
        "the refusal has to name the sandbox: {}",
        answer.stderr
    );
}

/// The request path returns the whole text of a real document, not a word of it.
///
/// The probe's `parse=` is the self-test's own answer; this is the same
/// documents arriving the way the server sends them, plan tag and bytes, with
/// the fixtures the parsers are tested against. A parser that stopped early, or
/// a limit that cut the text short, still contains the words these fixtures are
/// made of — the whole text is what says the request path is intact.
#[test]
fn the_child_returns_the_whole_text_of_a_real_document() {
    let args = ["--profile", "documents", "--min-level", "none"];
    let cases: [(u8, &[u8], &str); 4] = [
        (
            2,
            include_bytes!("fixtures/probe.pdf"),
            "nanofile sandbox probe",
        ),
        (
            3,
            include_bytes!("fixtures/probe.docx"),
            "nanofile sandbox probe",
        ),
        (
            4,
            include_bytes!("fixtures/probe.xlsx"),
            // A workbook's text arrives with its sheet name beside it.
            "Sheet1\nnanofile sandbox probe",
        ),
        (
            5,
            include_bytes!("fixtures/probe.pptx"),
            "nanofile sandbox probe",
        ),
    ];

    for (plan, bytes, expected) in cases {
        let answer = run_worker(&args, &document_request(plan, bytes));
        answer.finished("a packaged document");
        let (reply_tag, body) = answer.frame();
        assert_eq!(reply_tag, b'T', "plan {plan} is a document, not a refusal");
        let text = std::str::from_utf8(body).expect("extracted text is UTF-8");
        assert_eq!(
            text.trim(),
            expected,
            "plan {plan} must return its whole text"
        );
    }
}

/// The images child bounds what it will hold and refuses what it does not know.
#[test]
fn an_image_request_is_parsed_or_refused() {
    let args = ["--profile", "images", "--min-level", "none"];

    // The positive control: a real PNG decodes and comes back as an image,
    // fitted to the box the request names. The source is deliberately not
    // square: 32x16 fits a 16px box at 16x8, so the geometry says the frame was
    // decoded and resized rather than passed through.
    let answer = run_worker(&args, &image_request(b'T', 16, &a_png_of(32, 16)));
    answer.finished("a thumbnail request");
    let (tag, body) = answer.frame();
    assert!(
        tag == b'P' || tag == b'J',
        "a decodable image answers with an encoded one, not {:?}",
        tag as char
    );
    let decoded = image::load_from_memory(body).expect("the reply is an image");
    assert_eq!(
        (decoded.width(), decoded.height()),
        (16, 8),
        "a 32x16 source fits a 16px box at 16x8"
    );

    let answer = run_worker(&args, &image_request(b'?', 16, &a_png()));
    answer.finished("an unknown operation");
    assert_eq!(answer.refusal(), "unknown image operation");

    // The second half of the size bound the parent applies before it sends
    // anything: a request that reached the child over any other path still
    // cannot make it hold an unbounded buffer. The literal is
    // `jobs/images::MAX_IMAGE_BYTES`, spelled out because a wire bound is what
    // this pins.
    let oversized = image_request(b'T', 16, &vec![0u8; 32 * 1024 * 1024 + 1]);
    let answer = run_worker(&args, &oversized);
    answer.finished("an oversized image");
    assert_eq!(answer.refusal(), "image over the size cap");
}

/// The media child reads the one file it was granted, whatever the request
/// says.
#[test]
fn a_media_request_cannot_name_its_own_source() {
    let helper = env!("CARGO_BIN_EXE_nanofile");
    let granted = std::env::temp_dir().join("nanofile-protocol-granted.bin");
    let args = [
        "--profile",
        "media",
        "--min-level",
        "none",
        "--ffmpeg",
        helper,
        "--src",
        granted.to_str().expect("a UTF-8 path"),
    ];

    // The positive control for the comparison below: a request naming the
    // granted path passes the authorization step, whatever the helper then
    // does with the arguments it is given. Without this, a child that refused
    // every media request would satisfy the assertions below.
    let answer = run_worker(
        &args,
        &media_request(b'V', 48, granted.as_os_str().as_encoded_bytes()),
    );
    answer.finished("a media request naming the grant");
    let reason = answer.refusal();
    for refusal in [
        "the request names a source this profile was not granted",
        "the media profile has no source grant",
        "the media profile has no helper grant",
    ] {
        assert!(
            !reason.contains(refusal),
            "the granted path must pass the authorization step: {reason}"
        );
    }

    // The same shape, naming a path the parent did not grant.
    let answer = run_worker(
        &args,
        &media_request(b'V', 48, b"/tmp/nanofile-not-granted.bin"),
    );
    answer.finished("a media request naming another file");
    assert_eq!(
        answer.refusal(),
        "the request names a source this profile was not granted"
    );

    let answer = run_worker(
        &[
            "--profile",
            "media",
            "--min-level",
            "none",
            "--ffmpeg",
            helper,
            "--src",
            granted.to_str().expect("a UTF-8 path"),
        ],
        &media_request(b'?', 48, granted.as_os_str().as_encoded_bytes()),
    );
    answer.finished("an unknown media kind");
    assert_eq!(answer.refusal(), "unknown media kind");

    // A path length the child will not read: refused before any path byte is
    // consumed, which is why the request here is the head alone.
    for path_len in [0u16, 4097] {
        let answer = run_worker(&args, &media_head(b'V', 48, path_len));
        answer.finished("a bad path length");
        assert_eq!(answer.refusal(), "bad media source path length");
    }

    // No source grant at all: nothing can be read, so nothing is decoded.
    let answer = run_worker(
        &[
            "--profile",
            "media",
            "--min-level",
            "none",
            "--ffmpeg",
            helper,
        ],
        &media_request(b'V', 48, b"/tmp/anything.bin"),
    );
    answer.finished("a media request with no source grant");
    assert_eq!(answer.refusal(), "the media profile has no source grant");

    // No helper grant: the profile has nothing to run.
    let answer = run_worker(
        &[
            "--profile",
            "media",
            "--min-level",
            "none",
            "--src",
            granted.to_str().expect("a UTF-8 path"),
        ],
        &media_request(b'V', 48, granted.as_os_str().as_encoded_bytes()),
    );
    answer.finished("a media request with no helper grant");
    assert_eq!(answer.refusal(), "the media profile has no helper grant");
}

/// A PNG of `width`×`height` pixels: small enough that no size limit is what
/// refuses it.
fn a_png_of(width: u32, height: u32) -> Vec<u8> {
    let mut image = image::RgbImage::new(width, height);
    for pixel in image.pixels_mut() {
        *pixel = image::Rgb([10, 20, 30]);
    }
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encode a PNG");
    bytes
}

/// A one-pixel PNG.
fn a_png() -> Vec<u8> {
    a_png_of(1, 1)
}
