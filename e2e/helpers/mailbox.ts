import fs from "node:fs";
import net from "node:net";
import path from "node:path";

/**
 * A minimal SMTP server for the e2e run, plus the mailbox the specs read.
 *
 * The product has no test transport on purpose — a "write the mail to disk"
 * mode would be a way to put password-reset links on a production filesystem —
 * so the suite runs a real SMTP conversation against this stub instead. That
 * also means the tests exercise the real client (EHLO, envelope, dot-stuffing,
 * quoted-printable bodies) rather than a shortcut.
 *
 * Messages are written as `.eml` files under `test-results/mail/`, which makes
 * them readable from the worker processes (a spec runs in a different process
 * than `globalSetup`) and leaves them behind as artifacts when a test fails.
 */

export const SMTP_PORT = 18025;
export const MAIL_DIR = path.join(process.cwd(), "test-results", "mail");

export interface MailMessage {
  file: string;
  from: string;
  to: string;
  subject: string;
  /** The decoded body, with quoted-printable soft breaks removed. */
  body: string;
  raw: string;
}

/** Forget every message captured so far. */
export function clearMailbox(): void {
  fs.rmSync(MAIL_DIR, { recursive: true, force: true });
  fs.mkdirSync(MAIL_DIR, { recursive: true });
}

/** Every captured message, oldest first. */
export function readMailbox(): MailMessage[] {
  let files: string[];
  try {
    files = fs.readdirSync(MAIL_DIR).filter((name) => name.endsWith(".eml"));
  } catch {
    return [];
  }
  files.sort();
  return files.map((name) => {
    const file = path.join(MAIL_DIR, name);
    return parseMail(file, fs.readFileSync(file, "utf-8"));
  });
}

/** Messages addressed to `to` (case-insensitive). */
export function mailFor(to: string): MailMessage[] {
  const wanted = to.trim().toLowerCase();
  return readMailbox().filter((mail) => mail.to.trim().toLowerCase() === wanted);
}

/**
 * Wait for a message matching the filter, returning it.
 *
 * Polls rather than sleeping a fixed time: delivery is queued and attempted in
 * the background (that is the point of the outbox), so its arrival is
 * asynchronous by design.
 */
export async function waitForMail(
  filter: { to?: string; subjectIncludes?: string; since?: number },
  timeoutMs = 15_000,
): Promise<MailMessage> {
  const deadline = Date.now() + timeoutMs;
  let seen: MailMessage[] = [];
  while (Date.now() < deadline) {
    seen = readMailbox().filter((mail) => {
      if (filter.to && mail.to.trim().toLowerCase() !== filter.to.trim().toLowerCase()) {
        return false;
      }
      if (filter.subjectIncludes && !mail.subject.includes(filter.subjectIncludes)) {
        return false;
      }
      if (filter.since !== undefined && fs.statSync(mail.file).mtimeMs < filter.since) {
        return false;
      }
      return true;
    });
    if (seen.length > 0) return seen[0];
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(
    `no mail matched ${JSON.stringify(filter)} within ${timeoutMs}ms; ` +
      `mailbox holds: ${readMailbox()
        .map((mail) => `${mail.to} "${mail.subject}"`)
        .join(", ") || "(nothing)"}`,
  );
}

/** The first URL in the message matching `pattern`. */
export function extractLink(mail: MailMessage, pattern: RegExp): string {
  const match = mail.body.match(pattern);
  if (!match) {
    throw new Error(
      `no link matching ${pattern} in ${path.basename(mail.file)}:\n${mail.body}`,
    );
  }
  return match[0];
}

function parseMail(file: string, raw: string): MailMessage {
  const split = raw.indexOf("\n\n");
  const headerBlock = split === -1 ? raw : raw.slice(0, split);
  const rawBody = split === -1 ? "" : raw.slice(split + 2);
  const headers = new Map<string, string>();
  let current = "";
  for (const line of headerBlock.split("\n")) {
    if (/^[ \t]/.test(line) && current) {
      // A folded header continues the previous one.
      headers.set(current, `${headers.get(current)} ${line.trim()}`);
      continue;
    }
    const colon = line.indexOf(":");
    if (colon === -1) continue;
    current = line.slice(0, colon).trim().toLowerCase();
    headers.set(current, line.slice(colon + 1).trim());
  }

  return {
    file,
    from: headers.get("from") ?? "",
    to: headers.get("to") ?? "",
    subject: decodeHeader(headers.get("subject") ?? ""),
    // Soft line breaks are an artifact of the encoding, and the link we want to
    // extract is usually split across two of them.
    body: rawBody.replace(/=\r?\n/g, ""),
    raw,
  };
}

/**
 * Decode an RFC 2047 encoded word.
 *
 * Only what the suite needs: a UTF-8 body or subject. Anything else is returned
 * verbatim so a test failure shows the raw value instead of mojibake.
 */
function decodeHeader(value: string): string {
  return value.replace(/=\?utf-8\?([bBqQ])\?([^?]*)\?=/g, (all, encoding, text) => {
    try {
      if (encoding.toLowerCase() === "b") {
        return Buffer.from(text, "base64").toString("utf-8");
      }
      return Buffer.from(
        text.replace(/_/g, " ").replace(/=([0-9A-Fa-f]{2})/g, (_, hex) =>
          String.fromCharCode(parseInt(hex, 16)),
        ),
        "binary",
      ).toString("utf-8");
    } catch {
      return all;
    }
  });
}

interface Session {
  from: string;
  recipients: string[];
  inData: boolean;
  data: string[];
}

/**
 * Start the stub.
 *
 * Deliberately never closed: it lives in the runner process, which is exactly
 * as long as the run, and the OS releases the socket when the run ends. That
 * avoids handing a handle between `globalSetup` and `globalTeardown`, which are
 * separate module graphs.
 */
export async function startMailServer(port = SMTP_PORT): Promise<void> {
  clearMailbox();
  let sequence = 0;

  const server = net.createServer((socket) => {
    socket.setEncoding("utf-8");
    const session: Session = { from: "", recipients: [], inData: false, data: [] };
    let buffer = "";

    socket.write("220 nanofile-e2e ESMTP\r\n");

    socket.on("data", (chunk: string) => {
      buffer += chunk;
      let index: number;
      while ((index = buffer.indexOf("\r\n")) !== -1) {
        const line = buffer.slice(0, index);
        buffer = buffer.slice(index + 2);

        if (session.inData) {
          if (line === ".") {
            session.inData = false;
            sequence += 1;
            store(sequence, session);
            session.recipients = [];
            session.data = [];
            socket.write("250 OK queued\r\n");
          } else {
            // Undo dot-stuffing, which the client applies to a body line that
            // starts with a dot.
            session.data.push(line.startsWith("..") ? line.slice(1) : line);
          }
          continue;
        }

        const upper = line.toUpperCase();
        if (upper.startsWith("EHLO") || upper.startsWith("HELO")) {
          // Advertise nothing: no STARTTLS (the suite configures `tls = none`),
          // no AUTH, no PIPELINING.
          socket.write("250-nanofile-e2e\r\n250 8BITMIME\r\n");
        } else if (upper.startsWith("MAIL FROM")) {
          session.from = angleAddress(line) ?? line;
          socket.write("250 OK\r\n");
        } else if (upper.startsWith("RCPT TO")) {
          const recipient = angleAddress(line);
          if (recipient) session.recipients.push(recipient);
          socket.write("250 OK\r\n");
        } else if (upper.startsWith("DATA")) {
          session.inData = true;
          socket.write("354 End data with <CR><LF>.<CR><LF>\r\n");
        } else if (upper.startsWith("QUIT")) {
          socket.write("221 Bye\r\n");
          socket.end();
        } else {
          socket.write("250 OK\r\n");
        }
      }
    });

    socket.on("error", () => socket.destroy());
  });

  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", () => resolve());
  });
}

function store(sequence: number, session: Session): void {
  fs.mkdirSync(MAIL_DIR, { recursive: true });
  const to = session.recipients[0] ?? "unknown";
  const safe = to.replace(/[^a-zA-Z0-9@._-]/g, "_");
  const name = `${String(sequence).padStart(4, "0")}-${Date.now()}-${safe}.eml`;
  const payload = [
    `From: ${session.from}`,
    `To: ${session.recipients.join(", ")}`,
    ...session.data,
  ].join("\n");
  fs.writeFileSync(path.join(MAIL_DIR, name), payload);
}

/** The address inside `MAIL FROM:<a@b>` / `RCPT TO:<a@b>`. */
function angleAddress(line: string): string | null {
  const match = line.match(/<([^>]*)>/);
  return match ? match[1] : null;
}
