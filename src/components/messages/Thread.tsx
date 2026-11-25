import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Alert, Box, Button, CircularProgress, IconButton, Link, TextField, Tooltip, Typography, alpha, useTheme } from "@mui/material";
import ArrowBackIcon from "@mui/icons-material/ArrowBack";
import PhoneIcon from "@mui/icons-material/Phone";
import SendIcon from "@mui/icons-material/Send";
import AttachFileIcon from "@mui/icons-material/AttachFile";
import CloseIcon from "@mui/icons-material/Close";
import DescriptionIcon from "@mui/icons-material/Description";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { threadId, useMessagingStore } from "../../stores/messagingStore";
import { notifyError } from "../../stores/toastStore";
import { placeCall } from "../../hooks/useSip";
import { dialUri } from "../../utils/sipUri";
import { log } from "../../utils/log";
import type { SipAccount } from "../../types/sip";
import type { Mailbox } from "../../utils/voicemail";
import {
  attachmentProblem,
  countSegments,
  formatNumber,
  isImage,
  MAX_SEGMENTS,
  messagingErrorText,
  emptyDraft,
  isDialable,
  sendable,
  threadKey,
  oldestFirst,
  personLabel,
  showsStatus,
  type Attachment,
  type Inbox,
  type SmsMessage,
} from "../../utils/messaging";

/** What a thread with nothing typed shows. */
const NO_DRAFT = emptyDraft();

/** How often an open thread asks again, so a delivery report shows up. */
const THREAD_POLL_MS = 15_000;

function time(createdAt: string): string {
  const date = new Date(createdAt);
  if (Number.isNaN(date.getTime())) return "";
  const t = date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return date.toDateString() === new Date().toDateString() ? t : `${date.toLocaleDateString([], { month: "short", day: "numeric" })} · ${t}`;
}

/** One picture or file of a text, fetched through the PBX on demand. */
function AttachmentView({ mailbox, attachment }: { mailbox: Mailbox; attachment: Attachment }) {
  const { t } = useTranslation();
  const loadAttachment = useMessagingStore((s) => s.loadAttachment);
  const [url, setUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const image = isImage(attachment.content_type);

  useEffect(() => {
    if (!image) return;
    let revoked = false;
    let made: string | null = null;
    loadAttachment(mailbox, attachment.id)
      .then((bytes) => {
        if (revoked) return;
        made = URL.createObjectURL(new Blob([bytes], { type: attachment.content_type }));
        setUrl(made);
      })
      .catch((e) => {
        log.warn("[messaging] attachment failed:", messagingErrorText(e));
        if (!revoked) setFailed(true);
      });
    return () => {
      revoked = true;
      if (made) URL.revokeObjectURL(made);
    };
  }, [mailbox, attachment.id, attachment.content_type, image, loadAttachment]);

  if (!image) {
    return (
      <Box sx={{ display: "flex", alignItems: "center", gap: 0.5, fontSize: "0.8rem" }}>
        <DescriptionIcon fontSize="small" /> {attachment.content_type}
      </Box>
    );
  }
  if (failed) return <Typography variant="caption">{t("messages.attachmentLoadFailed")}</Typography>;
  if (!url) return <CircularProgress size={16} />;
  return <img src={url} alt={t("messages.picture")} style={{ maxWidth: "100%", maxHeight: 240, borderRadius: 12, display: "block" }} />;
}

function Bubble({ message, mailbox }: { message: SmsMessage; mailbox: Mailbox }) {
  const { t } = useTranslation();
  const theme = useTheme();
  const mine = message.direction === "outbound";
  const failed = message.status === "failed";
  const statusLabel = { pending: t("messages.status.pending"), sent: t("messages.status.sent"), delivered: t("messages.status.delivered"), failed: t("messages.status.failed") }[
    message.status as "pending" | "sent" | "delivered" | "failed"
  ];
  return (
    <Box sx={{ alignSelf: mine ? "flex-end" : "flex-start", maxWidth: "75%", display: "flex", flexDirection: "column", alignItems: mine ? "flex-end" : "flex-start", gap: 0.25 }}>
      <Box
        sx={{
          px: 1.5,
          py: 1,
          borderRadius: "16px",
          bgcolor: failed ? alpha(theme.palette.error.main, 0.12) : mine ? "primary.main" : alpha(theme.palette.text.primary, 0.08),
          color: failed ? "text.primary" : mine ? "primary.contrastText" : "text.primary",
          display: "flex",
          flexDirection: "column",
          gap: 0.75,
        }}
      >
        {message.attachments.map((a) => (
          <AttachmentView key={a.id} mailbox={mailbox} attachment={a} />
        ))}
        {message.body && (
          <Typography variant="body2" sx={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>
            {message.body}
          </Typography>
        )}
      </Box>
      <Typography variant="caption" sx={{ color: failed ? "error.main" : "text.secondary" }}>
        {time(message.created_at)}
        {showsStatus(message) && statusLabel ? ` · ${statusLabel}` : ""}
        {failed ? ` · ${message.error_message ? t("messages.failedWith", { error: message.error_message }) : t("messages.failedNoReason")}` : ""}
      </Typography>
    </Box>
  );
}

interface ThreadProps {
  mailbox: Mailbox;
  account: SipAccount;
  inbox: Inbox;
  /** E.164. */
  number: string;
  onBack: () => void;
}

/**
 * One conversation: the texts oldest first, the person's number (tap it to
 * call), and a composer that counts segments the way the PBX enforces them and
 * attaches one picture. A refusal's sentence comes from the PBX untouched.
 */
export function Thread({ mailbox, account, inbox, number, onBack }: ThreadProps) {
  const { t } = useTranslation();
  const contacts = useAppStore((s) => s.contacts);
  const defaultCountry = useAppStore((s) => s.defaultCountry);
  const thread = useMessagingStore((s) => s.byAccount[mailbox.accountId]?.threads[threadId(inbox.did_id, number)]);
  const loadThread = useMessagingStore((s) => s.loadThread);
  const sendDraft = useMessagingStore((s) => s.sendDraft);
  const setDraft = useMessagingStore((s) => s.setDraft);
  const upload = useMessagingStore((s) => s.upload);
  const markRead = useMessagingStore((s) => s.markRead);
  const setOpen = useMessagingStore((s) => s.setOpen);

  // The draft lives in the store, so leaving the thread and coming back (or
  // the list re-rendering it) keeps the text, the picture and the idempotency
  // key of a send that may still be in flight.
  const draftKey = threadKey(mailbox.accountId, inbox.did_id, number);
  const draft = useMessagingStore((s) => s.drafts[draftKey]) ?? NO_DRAFT;
  const { body, attachment, sending } = draft;
  const [uploading, setUploading] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const bottom = useRef<HTMLDivElement>(null);

  const reload = useCallback(
    () =>
      loadThread(mailbox, inbox.did_id, number)
        .then(() => setLoadError(null))
        .catch((e) => setLoadError(messagingErrorText(e))),
    [loadThread, mailbox, inbox.did_id, number],
  );

  // On screen: told to the store (so its notification is held back), read now
  // and again whenever something newer lands, and refreshed on a timer.
  useEffect(() => {
    setOpen({ accountId: mailbox.accountId, didId: inbox.did_id, number });
    void reload();
    const timer = setInterval(() => void reload(), THREAD_POLL_MS);
    return () => {
      clearInterval(timer);
      setOpen(null);
    };
  }, [mailbox.accountId, inbox.did_id, number, reload, setOpen]);

  const messages = useMemo(() => oldestFirst(thread?.messages ?? []), [thread?.messages]);
  const latest = messages.length > 0 ? messages[messages.length - 1].created_at : "";
  useEffect(() => {
    markRead(mailbox.accountId, inbox.did_id, number);
    bottom.current?.scrollIntoView?.({ block: "end" });
  }, [latest, mailbox.accountId, inbox.did_id, number, markRead]);

  const label = personLabel(number, contacts);
  // Counted as sent: the PBX is given the trimmed text, so that is what costs segments.
  const count = countSegments(sendable(draft));
  const optedOut = thread?.opted_out ?? false;
  const canSend = inbox.enabled && !optedOut;
  const canSubmit = canSend && !sending && !uploading && !count.overLimit && (sendable(draft).length > 0 || attachment !== null);
  const dialable = isDialable(number);

  const call = () => {
    if (!dialable) return;
    placeCall({ uri: dialUri(number, account, defaultCountry), accountId: account.id, remoteName: label !== formatNumber(number) ? label : undefined }).catch((e) =>
      notifyError(messagingErrorText(e)),
    );
  };

  const pick = async (file: File | undefined) => {
    if (!file) return;
    const bad = attachmentProblem(file);
    if (bad) {
      setProblem(bad === "type" ? t("messages.attachBadType") : t("messages.attachTooBig"));
      return;
    }
    setProblem(null);
    setUploading(true);
    try {
      setDraft(draftKey, { attachment: await upload(mailbox, file) });
    } catch (e) {
      setProblem(t("messages.attachFailed", { error: messagingErrorText(e) }));
    } finally {
      setUploading(false);
    }
  };

  const submit = () => {
    setProblem(null);
    void sendDraft(mailbox, inbox.did_id, number);
  };

  return (
    <Box sx={{ height: "100%", display: "flex", flexDirection: "column" }}>
      <Box sx={{ display: "flex", alignItems: "center", gap: 1, px: 2, pt: 2, pb: 1 }}>
        <IconButton size="small" onClick={onBack} aria-label={t("messages.back")}>
          <ArrowBackIcon fontSize="small" />
        </IconButton>
        <Box sx={{ flex: 1, minWidth: 0 }}>
          <Typography variant="h6" sx={{ fontWeight: 600 }} noWrap>
            {label}
          </Typography>
          {dialable ? (
            <Link component="button" type="button" underline="hover" variant="caption" onClick={call} sx={{ color: "text.secondary" }} aria-label={t("messages.call", { number: formatNumber(number) })}>
              {formatNumber(number)}
            </Link>
          ) : (
            <Typography variant="caption" sx={{ color: "text.secondary", display: "block" }}>
              {number}
            </Typography>
          )}
        </Box>
        {dialable && (
          <Tooltip title={t("messages.call", { number: formatNumber(number) })}>
            <IconButton size="small" color="primary" onClick={call} aria-label={t("messages.call", { number: formatNumber(number) })}>
              <PhoneIcon fontSize="small" />
            </IconButton>
          </Tooltip>
        )}
      </Box>

      <Box sx={{ flex: 1, overflow: "auto", px: 2, py: 1, display: "flex", flexDirection: "column", gap: 1 }}>
        {loadError && <Alert severity="error">{t("messages.loadFailed")} {loadError}</Alert>}
        {!thread && !loadError && <CircularProgress size={24} sx={{ alignSelf: "center", mt: 4 }} />}
        {thread && messages.length === 0 && (
          <Typography variant="body2" sx={{ color: "text.secondary", textAlign: "center", mt: 4 }}>
            {t("messages.noThread")}
          </Typography>
        )}
        {messages.map((m) => (
          <Bubble key={m.id} message={m} mailbox={mailbox} />
        ))}
        <div ref={bottom} />
      </Box>

      <Box sx={{ px: 2, pb: 2, pt: 1, display: "flex", flexDirection: "column", gap: 1 }}>
        {optedOut && <Alert severity="warning">{t("messages.optedOut")}</Alert>}
        {!inbox.enabled && <Alert severity="info">{t("messages.inboxOff")}</Alert>}
        {draft.error && <Alert severity="error">{t("messages.sendFailed", { error: draft.error })}</Alert>}
        {problem && <Alert severity="error" onClose={() => setProblem(null)}>{problem}</Alert>}
        {attachment && (
          <Box sx={{ display: "flex", alignItems: "center", gap: 1, fontSize: "0.85rem" }}>
            <DescriptionIcon fontSize="small" /> {attachment.content_type} · {Math.max(1, Math.round(attachment.size_bytes / 1024))} KB
            <IconButton size="small" onClick={() => setDraft(draftKey, { attachment: null })} aria-label={t("messages.removeAttachment")}>
              <CloseIcon fontSize="small" />
            </IconButton>
          </Box>
        )}
        <Box sx={{ display: "flex", alignItems: "flex-end", gap: 1 }}>
          <input ref={fileInput} type="file" hidden accept="image/jpeg,image/png,image/gif,video/mp4,application/pdf" onChange={(e) => { void pick(e.target.files?.[0]); e.target.value = ""; }} />
          <Tooltip title={t("messages.attach")}>
            <span>
              <IconButton onClick={() => fileInput.current?.click()} disabled={!canSend || uploading || attachment !== null} aria-label={t("messages.attach")}>
                {uploading ? <CircularProgress size={20} /> : <AttachFileIcon />}
              </IconButton>
            </span>
          </Tooltip>
          <TextField
            fullWidth
            multiline
            maxRows={5}
            size="small"
            placeholder={t("messages.placeholder")}
            value={body}
            disabled={!canSend || sending}
            onChange={(e) => setDraft(draftKey, { body: e.target.value })}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                if (canSubmit) submit();
              }
            }}
          />
          <Button variant="contained" endIcon={<SendIcon />} disabled={!canSubmit} onClick={() => submit()}>
            {t("messages.send")}
          </Button>
        </Box>
        {sendable(draft).length > 0 && (
          <Typography variant="caption" sx={{ color: count.overLimit ? "error.main" : "text.secondary" }} data-testid="segment-count">
            {t("messages.segments", { count: count.segments })} · {count.encoding === "gsm7" ? t("messages.encodingGsm") : t("messages.encodingUcs")} · {t("messages.left", { count: count.remaining })}
            {count.overLimit ? ` · ${t("messages.tooLong", { max: MAX_SEGMENTS })}` : ""}
          </Typography>
        )}
      </Box>
    </Box>
  );
}
