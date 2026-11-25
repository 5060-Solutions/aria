import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  Avatar,
  Box,
  Button,
  Chip,
  CircularProgress,
  FormControl,
  IconButton,
  InputLabel,
  List,
  ListItemAvatar,
  ListItemButton,
  ListItemText,
  MenuItem,
  Select,
  TextField,
  Tooltip,
  Typography,
  alpha,
  useMediaQuery,
  useTheme,
} from "@mui/material";
import ChatIcon from "@mui/icons-material/ChatBubbleOutlined";
import RefreshIcon from "@mui/icons-material/Refresh";
import ArrowBackIcon from "@mui/icons-material/ArrowBack";
import EditIcon from "@mui/icons-material/EditOutlined";
import PhoneIcon from "@mui/icons-material/Phone";
import SendIcon from "@mui/icons-material/Send";
import AttachFileIcon from "@mui/icons-material/AttachFile";
import FiberManualRecordIcon from "@mui/icons-material/FiberManualRecord";
import PersonIcon from "@mui/icons-material/PersonOutlined";
import type { CountryCode } from "libphonenumber-js";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { useMessagingStore, threadKey, type PbxError } from "../../stores/messagingStore";
import { errorText, notifyError } from "../../stores/toastStore";
import { placeCall } from "../../hooks/useSip";
import { dialUri } from "../../utils/sipUri";
import { log } from "../../utils/log";
import { mailboxFor, formatVoicemailTime, type Mailbox } from "../../utils/voicemail";
import { dialerMatches, formatNumber, uriUser } from "../../utils/callerDisplay";
import {
  MAX_SEGMENTS,
  conversationName,
  normalizeRecipient,
  smsSegments,
  type Conversation,
  type Inbox,
  type SmsAttachment,
  type SmsMessage,
} from "../../utils/messaging";
import type { Contact } from "../../types/sip";

/** How often the open inbox and thread are re-read while the view is up. */
const VIEW_POLL_MS = 15_000;

/** Two letters of a name; a bare number gets a person glyph instead of digits. */
/** A row's time: the clock today, otherwise the date alone; the thread has the full stamp. */
function listTime(iso: string, now: Date = new Date()): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  if (date.toDateString() === now.toDateString()) return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return date.toLocaleDateString([], { month: "short", day: "numeric" });
}

function initials(primary: string): ReactNode {
  if (/^[+\d\s().-]+$/.test(primary)) return <PersonIcon fontSize="small" />;
  const letters = primary.replace(/[^\p{L}\p{N}]/gu, "");
  return letters ? letters.substring(0, 2).toUpperCase() : <PersonIcon fontSize="small" />;
}

/**
 * Texts on the account's messaging numbers: the conversations of one inbox,
 * a thread with its pictures, and a composer. One pane on the phone-sized
 * window (list, then thread with a back arrow), two side by side when the
 * window is wide enough.
 */
export function Messages() {
  const { t } = useTranslation();
  const theme = useTheme();
  const wide = useMediaQuery("(min-width: 840px)");
  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const contacts = useAppStore((s) => s.contacts);
  const defaultCountry = useAppStore((s) => s.defaultCountry) as CountryCode;
  const account = accounts.find((a) => a.id === activeAccountId);
  const mailbox = useMemo(() => mailboxFor(account), [account]);

  const inboxes = useMessagingStore((s) => (mailbox ? s.byAccount[mailbox.accountId]?.inboxes : undefined)) ?? [];
  const [chosenDid, setChosenDid] = useState<string | null>(null);
  const inbox: Inbox | undefined = inboxes.find((i) => i.didId === chosenDid) ?? inboxes.find((i) => i.enabled) ?? inboxes[0];
  const didId = inbox?.didId ?? null;

  const conversationsState = useMessagingStore((s) => (didId ? s.conversations[didId] : undefined));
  const loadConversations = useMessagingStore((s) => s.loadConversations);
  const openThread = useMessagingStore((s) => s.openThread);
  const seen = useMessagingStore((s) => s.seen);

  const [number, setNumber] = useState<string | null>(null);
  const [composing, setComposing] = useState(false);

  // Refresh the open inbox and thread while the view is up.
  useEffect(() => {
    if (!mailbox || !didId) return;
    const tick = () => {
      void loadConversations(mailbox, didId);
      if (number) void openThread(mailbox, didId, number);
    };
    const timer = window.setInterval(tick, VIEW_POLL_MS);
    return () => window.clearInterval(timer);
  }, [mailbox, didId, number, loadConversations, openThread]);

  const open = useCallback(
    (to: string) => {
      if (!mailbox || !didId) return;
      setComposing(false);
      setNumber(to);
      void openThread(mailbox, didId, to);
    },
    [mailbox, didId, openThread],
  );

  const back = () => {
    setNumber(null);
    setComposing(false);
  };

  const list = conversationsState?.list ?? [];
  const showList = wide || (!number && !composing);
  const showThread = wide || number !== null || composing;

  const header = (
    <Box sx={{ display: "flex", alignItems: "center", gap: 1, px: 2, pt: 2, pb: 1 }}>
      {!wide && (number !== null || composing) ? (
        <IconButton size="small" onClick={back} aria-label={t("messages.back")}>
          <ArrowBackIcon fontSize="small" />
        </IconButton>
      ) : (
        <ChatIcon sx={{ color: "primary.main" }} />
      )}
      <Typography variant="h6" sx={{ fontWeight: 600, flex: 1 }} noWrap>
        {t("messages.title")}
      </Typography>
      {mailbox && didId && (
        <>
          <Tooltip title={t("messages.compose")}>
            <IconButton
              size="small"
              onClick={() => {
                setNumber(null);
                setComposing(true);
              }}
              aria-label={t("messages.compose")}
              disabled={inbox ? !inbox.enabled : true}
            >
              <EditIcon fontSize="small" />
            </IconButton>
          </Tooltip>
          <Tooltip title={t("messages.refresh")}>
            <span>
              <IconButton
                size="small"
                onClick={() => {
                  void loadConversations(mailbox, didId);
                  if (number) void openThread(mailbox, didId, number);
                }}
                disabled={conversationsState?.loading ?? false}
                aria-label={t("messages.refresh")}
              >
                {conversationsState?.loading ? <CircularProgress size={18} /> : <RefreshIcon fontSize="small" />}
              </IconButton>
            </span>
          </Tooltip>
        </>
      )}
    </Box>
  );

  const notice = (text: string, action?: { label: string; onClick: () => void }) => (
    <Box
      sx={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: 1.5,
        color: "text.secondary",
        px: 3,
        textAlign: "center",
      }}
    >
      <Typography variant="body2">{text}</Typography>
      {action && (
        <Button size="small" variant="outlined" onClick={action.onClick}>
          {action.label}
        </Button>
      )}
    </Box>
  );

  if (!mailbox || !account || !inbox || !didId) {
    // Only reachable if the active account changed under an open view.
    return (
      <Box sx={{ height: "100%", display: "flex", flexDirection: "column" }}>
        {header}
        {notice(t("messages.none"))}
      </Box>
    );
  }

  const inboxPicker =
    inboxes.length > 1 ? (
      <FormControl size="small" fullWidth sx={{ px: 2, pb: 1 }}>
        <InputLabel id="messages-inbox-label" sx={{ ml: 2 }}>
          {t("messages.inbox")}
        </InputLabel>
        <Select
          labelId="messages-inbox-label"
          label={t("messages.inbox")}
          value={inbox.didId}
          onChange={(e) => {
            setChosenDid(e.target.value);
            setNumber(null);
            setComposing(false);
          }}
        >
          {inboxes.map((i) => (
            <MenuItem key={i.didId} value={i.didId}>
              {formatNumber(i.number, defaultCountry)}
              {!i.enabled && ` · ${t("messages.readOnly")}`}
            </MenuItem>
          ))}
        </Select>
      </FormControl>
    ) : (
      <Typography variant="caption" sx={{ px: 2, pb: 1, color: "text.secondary" }} noWrap>
        {t("messages.from", { number: formatNumber(inbox.number, defaultCountry) })}
        {!inbox.enabled && ` · ${t("messages.readOnly")}`}
      </Typography>
    );

  const errorNotice = (error: PbxError, retry: () => void) => {
    const text =
      error.kind === "unauthorized" || error.kind === "noPassword"
        ? t("messages.signInFailed")
        : `${t("messages.loadFailed")} ${error.message}`;
    return notice(text, { label: t("messages.retry"), onClick: retry });
  };

  const listPane = (
    <Box sx={{ display: "flex", flexDirection: "column", minHeight: 0, minWidth: 0, flex: wide ? "0 0 320px" : 1, borderRight: wide ? `1px solid ${alpha(theme.palette.divider, 0.12)}` : "none" }}>
      {inboxPicker}
      {conversationsState?.error && !list.length ? (
        errorNotice(conversationsState.error, () => void loadConversations(mailbox, didId))
      ) : !conversationsState?.loaded ? (
        <Box sx={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center" }}>
          <CircularProgress size={28} />
        </Box>
      ) : list.length === 0 ? (
        notice(t("messages.empty"), inbox.enabled ? { label: t("messages.compose"), onClick: () => setComposing(true) } : undefined)
      ) : (
        <List sx={{ flex: 1, overflow: "auto", px: 1, py: 0 }}>
          {list.map((c) => (
            <ConversationRow
              key={c.remoteNumber}
              conversation={c}
              contacts={contacts}
              defaultCountry={defaultCountry}
              selected={c.remoteNumber === number}
              unseen={c.lastDirection === "inbound" && (!seen[threadKey(didId, c.remoteNumber)] || Date.parse(c.lastAt) > Date.parse(seen[threadKey(didId, c.remoteNumber)]))}
              onOpen={() => open(c.remoteNumber)}
            />
          ))}
        </List>
      )}
    </Box>
  );

  const threadPane = composing ? (
    <Compose
      mailbox={mailbox}
      didId={didId}
      contacts={contacts}
      defaultCountry={defaultCountry}
      onStart={open}
    />
  ) : number ? (
    <Thread
      key={threadKey(didId, number)}
      mailbox={mailbox}
      didId={didId}
      number={number}
      inbox={inbox}
      contacts={contacts}
      defaultCountry={defaultCountry}
      ownDomain={account.domain}
      onCall={(label) =>
        placeCall({ uri: dialUri(number, account, defaultCountry), accountId: account.id, remoteName: label }).catch((e) =>
          notifyError(errorText(e)),
        )
      }
    />
  ) : (
    notice(t("messages.pick"))
  );

  return (
    <Box sx={{ height: "100%", display: "flex", flexDirection: "column" }}>
      {header}
      <Box sx={{ flex: 1, display: "flex", minHeight: 0 }}>
        {showList && listPane}
        {showThread && (
          <Box sx={{ flex: 1, display: "flex", flexDirection: "column", minWidth: 0, minHeight: 0 }}>{threadPane}</Box>
        )}
      </Box>
    </Box>
  );
}

function ConversationRow({
  conversation,
  contacts,
  defaultCountry,
  selected,
  unseen,
  onOpen,
}: {
  conversation: Conversation;
  contacts: ReadonlyArray<Contact>;
  defaultCountry: CountryCode;
  selected: boolean;
  unseen: boolean;
  onOpen: () => void;
}) {
  const { t } = useTranslation();
  const theme = useTheme();
  const name = conversationName(conversation.remoteNumber, contacts, defaultCountry);
  const preview = conversation.lastBody.trim() || (conversation.lastHasMedia ? t("messages.picture") : "");
  return (
    <ListItemButton onClick={onOpen} selected={selected} sx={{ borderRadius: "16px", mb: 0.5, py: 1 }}>
      <ListItemAvatar>
        <Avatar
          sx={{
            width: 40,
            height: 40,
            bgcolor: alpha(theme.palette.primary.main, unseen ? 0.18 : 0.08),
            color: "primary.main",
            fontSize: "0.9rem",
          }}
        >
          {initials(name.primary)}
        </Avatar>
      </ListItemAvatar>
      <ListItemText
        sx={{ minWidth: 0 }}
        primary={
          <Box sx={{ display: "flex", alignItems: "center", gap: 1, minWidth: 0 }}>
            {unseen && <FiberManualRecordIcon sx={{ fontSize: 10, color: "primary.main", flexShrink: 0 }} titleAccess={t("messages.unseen")} />}
            <Box component="span" sx={{ fontWeight: unseen ? 600 : 400, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", minWidth: 0 }}>
              {name.primary}
            </Box>
            <Box component="span" sx={{ ml: "auto", flexShrink: 0, fontSize: "0.75rem", color: "text.secondary" }}>
              {listTime(conversation.lastAt)}
            </Box>
          </Box>
        }
        secondary={
          <Box component="span" sx={{ display: "block", fontSize: "0.8rem", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", opacity: unseen ? 1 : 0.7 }}>
            {conversation.lastDirection === "outbound" && `${t("messages.you")}: `}
            {preview}
          </Box>
        }
      />
    </ListItemButton>
  );
}

function Compose({
  mailbox,
  didId,
  contacts,
  defaultCountry,
  onStart,
}: {
  mailbox: Mailbox;
  didId: string;
  contacts: ReadonlyArray<Contact>;
  defaultCountry: CountryCode;
  onStart: (number: string) => void;
}) {
  const { t } = useTranslation();
  const theme = useTheme();
  const [input, setInput] = useState("");
  const matches = useMemo(() => dialerMatches(input, contacts, 6), [input, contacts]);
  const typed = normalizeRecipient(input, defaultCountry);
  void mailbox;
  void didId;

  const pick = (contact: Contact) => {
    const candidate = normalizeRecipient(contact.phone ?? "", defaultCountry) ?? normalizeRecipient(uriUser(contact.uri), defaultCountry);
    if (candidate) onStart(candidate);
    else notifyError(t("messages.notTextable", { name: contact.name }));
  };

  return (
    <Box sx={{ p: 2, display: "flex", flexDirection: "column", gap: 1.5 }}>
      <TextField
        autoFocus
        fullWidth
        size="small"
        label={t("messages.to")}
        placeholder={t("messages.toPlaceholder")}
        value={input}
        onChange={(e) => setInput(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            if (typed) onStart(typed);
            else if (matches[0]) pick(matches[0]);
          }
        }}
        helperText={input && !typed && matches.length === 0 ? t("messages.notANumber") : " "}
      />
      {matches.length > 0 && (
        <Box role="listbox" aria-label={t("dialer.matches")} sx={{ borderRadius: "14px", border: `1px solid ${alpha(theme.palette.divider, 0.3)}`, overflow: "hidden" }}>
          {matches.map((contact) => (
            <Box
              key={contact.id}
              role="option"
              aria-selected={false}
              tabIndex={0}
              onClick={() => pick(contact)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  pick(contact);
                }
              }}
              sx={{ display: "flex", alignItems: "center", gap: 1.25, px: 1.5, py: 0.75, cursor: "pointer", "&:hover": { bgcolor: alpha(theme.palette.primary.main, 0.14) } }}
            >
              <Avatar sx={{ width: 30, height: 30, bgcolor: alpha(theme.palette.primary.main, 0.14), color: "primary.main", fontSize: "0.75rem" }}>
                {initials(contact.name)}
              </Avatar>
              <Box sx={{ minWidth: 0 }}>
                <Typography variant="body2" noWrap sx={{ fontWeight: 500 }}>
                  {contact.name}
                </Typography>
                <Typography variant="caption" noWrap sx={{ color: "text.secondary", display: "block" }}>
                  {formatNumber(contact.phone || uriUser(contact.uri), defaultCountry)}
                </Typography>
              </Box>
            </Box>
          ))}
        </Box>
      )}
      {typed && (
        <Button variant="contained" size="small" onClick={() => onStart(typed)} sx={{ alignSelf: "flex-start" }}>
          {t("messages.startWith", { number: formatNumber(typed, defaultCountry) })}
        </Button>
      )}
    </Box>
  );
}

function Thread({
  mailbox,
  didId,
  number,
  inbox,
  contacts,
  defaultCountry,
  onCall,
}: {
  mailbox: Mailbox;
  didId: string;
  number: string;
  inbox: Inbox;
  contacts: ReadonlyArray<Contact>;
  defaultCountry: CountryCode;
  ownDomain: string;
  onCall: (label: string | undefined) => void;
}) {
  const { t } = useTranslation();
  const theme = useTheme();
  const key = threadKey(didId, number);
  const state = useMessagingStore((s) => s.threads[key]);
  const openThread = useMessagingStore((s) => s.openThread);
  const loadOlder = useMessagingStore((s) => s.loadOlder);
  const send = useMessagingStore((s) => s.send);
  const name = conversationName(number, contacts, defaultCountry);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [refused, setRefused] = useState<string | null>(null);
  const bottom = useRef<HTMLDivElement | null>(null);
  const messages = useMemo(() => [...(state?.thread.messages ?? [])].reverse(), [state?.thread.messages]);
  const count = messages.length;

  useEffect(() => {
    bottom.current?.scrollIntoView({ block: "end" });
  }, [count]);

  const segments = smsSegments(draft);
  const optedOut = state?.thread.optedOut ?? false;
  const canSend = inbox.enabled && !optedOut && segments.segments > 0 && segments.segments <= MAX_SEGMENTS && !sending;

  const submit = async () => {
    if (!canSend) return;
    setSending(true);
    setRefused(null);
    try {
      await send(mailbox, didId, number, draft.trim());
      setDraft("");
    } catch (e) {
      const err = e as PbxError;
      log.warn("[messages] send failed:", err.kind, err.message);
      setRefused(err.kind === "rejected" ? err.message : `${t("messages.sendFailed")} ${err.message}`);
    } finally {
      setSending(false);
    }
  };

  return (
    <>
      <Box sx={{ display: "flex", alignItems: "center", gap: 1.5, px: 2, py: 1, borderBottom: `1px solid ${alpha(theme.palette.divider, 0.12)}` }}>
        <Avatar sx={{ width: 36, height: 36, bgcolor: alpha(theme.palette.primary.main, 0.1), color: "primary.main", fontSize: "0.85rem" }}>
          {initials(name.primary)}
        </Avatar>
        <Box sx={{ flex: 1, minWidth: 0 }}>
          <Typography variant="subtitle1" sx={{ fontWeight: 600, lineHeight: 1.2 }} noWrap>
            {name.primary}
          </Typography>
          {name.secondary && (
            <Typography variant="caption" sx={{ color: "text.secondary" }} noWrap>
              {name.secondary}
            </Typography>
          )}
        </Box>
        <Tooltip title={t("messages.call")}>
          <IconButton size="small" onClick={() => onCall(name.secondary ? name.primary : undefined)} aria-label={t("messages.call")}>
            <PhoneIcon fontSize="small" />
          </IconButton>
        </Tooltip>
      </Box>

      <Box sx={{ flex: 1, overflow: "auto", px: 2, py: 1.5, display: "flex", flexDirection: "column", gap: 0.75 }}>
        {state && !state.exhausted && count > 0 && (
          <Button size="small" onClick={() => void loadOlder(mailbox, didId, number)} disabled={state.loading} sx={{ alignSelf: "center" }}>
            {t("messages.older")}
          </Button>
        )}
        {state?.error && count === 0 ? (
          <Box sx={{ flex: 1, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", gap: 1, color: "text.secondary", textAlign: "center" }}>
            <Typography variant="body2">
              {t("messages.loadFailed")} {state.error.message}
            </Typography>
            <Button size="small" variant="outlined" onClick={() => void openThread(mailbox, didId, number)}>
              {t("messages.retry")}
            </Button>
          </Box>
        ) : state?.loading && count === 0 ? (
          <Box sx={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center" }}>
            <CircularProgress size={24} />
          </Box>
        ) : count === 0 ? (
          <Box sx={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center", color: "text.secondary", textAlign: "center" }}>
            <Typography variant="body2">{t("messages.noMessagesYet")}</Typography>
          </Box>
        ) : (
          messages.map((m) => <Bubble key={m.id} message={m} mailbox={mailbox} />)
        )}
        <div ref={bottom} />
      </Box>

      <Box sx={{ px: 2, pb: 2, pt: 1, borderTop: `1px solid ${alpha(theme.palette.divider, 0.12)}` }}>
        {optedOut ? (
          <Typography variant="body2" sx={{ color: "text.secondary" }}>
            {t("messages.optedOut")}
          </Typography>
        ) : !inbox.enabled ? (
          <Typography variant="body2" sx={{ color: "text.secondary" }}>
            {t("messages.readOnlyInbox")}
          </Typography>
        ) : (
          <>
            <Box sx={{ display: "flex", alignItems: "flex-end", gap: 1 }}>
              <TextField
                fullWidth
                multiline
                maxRows={5}
                size="small"
                placeholder={t("messages.placeholder")}
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey) {
                    e.preventDefault();
                    void submit();
                  }
                }}
                slotProps={{ htmlInput: { "aria-label": t("messages.placeholder") } }}
              />
              <Tooltip title={t("messages.send")}>
                <span>
                  <IconButton color="primary" onClick={() => void submit()} disabled={!canSend} aria-label={t("messages.send")}>
                    {sending ? <CircularProgress size={20} /> : <SendIcon />}
                  </IconButton>
                </span>
              </Tooltip>
            </Box>
            <Box sx={{ display: "flex", justifyContent: "space-between", mt: 0.5, minHeight: 18 }}>
              <Typography variant="caption" sx={{ color: refused ? "error.main" : "text.secondary" }} noWrap>
                {refused ?? (segments.segments > MAX_SEGMENTS ? t("messages.tooLong", { max: MAX_SEGMENTS }) : "")}
              </Typography>
              {segments.segments > 0 && (segments.segments > 1 || segments.chars > segments.perSegment - 40) && (
                <Typography variant="caption" sx={{ color: segments.segments > MAX_SEGMENTS ? "error.main" : "text.secondary", flexShrink: 0 }}>
                  {segments.chars}/{segments.perSegment} · {segments.segments}/{MAX_SEGMENTS}
                </Typography>
              )}
            </Box>
          </>
        )}
      </Box>
    </>
  );
}

function Bubble({ message, mailbox }: { message: SmsMessage; mailbox: Mailbox }) {
  const { t } = useTranslation();
  const theme = useTheme();
  const mine = message.direction === "outbound";
  const failed = message.status === "failed";
  const status =
    failed
      ? `${t("messages.failed")}${message.errorMessage ? ` · ${message.errorMessage}` : ""}`
      : message.status === "pending"
        ? t("messages.sending")
        : message.status === "delivered"
          ? t("messages.delivered")
          : null;
  return (
    <Box sx={{ display: "flex", flexDirection: "column", alignItems: mine ? "flex-end" : "flex-start", maxWidth: "100%" }}>
      <Box
        sx={{
          maxWidth: "80%",
          px: 1.5,
          py: 1,
          borderRadius: mine ? "16px 16px 4px 16px" : "16px 16px 16px 4px",
          bgcolor: mine ? "primary.main" : alpha(theme.palette.text.primary, 0.08),
          color: mine ? "primary.contrastText" : "text.primary",
          border: failed ? `1px solid ${theme.palette.error.main}` : "none",
          whiteSpace: "pre-wrap",
          wordBreak: "break-word",
          fontSize: "0.9rem",
        }}
      >
        {message.attachments.map((a) => (
          <AttachmentView key={a.id} attachment={a} mailbox={mailbox} />
        ))}
        {message.body}
      </Box>
      <Typography variant="caption" sx={{ color: failed ? "error.main" : "text.secondary", mt: 0.25, px: 0.5 }}>
        {formatVoicemailTime(message.createdAt)}
        {message.autoReply && ` · ${t("messages.autoReply")}`}
        {status && ` · ${status}`}
      </Typography>
    </Box>
  );
}

/** An MMS picture, fetched through the PBX with the session and shown from a blob URL. */
function AttachmentView({ attachment, mailbox }: { attachment: SmsAttachment; mailbox: Mailbox }) {
  const { t } = useTranslation();
  const load = useMessagingStore((s) => s.attachment);
  const [url, setUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const image = attachment.contentType.startsWith("image/");

  useEffect(() => {
    if (!image) return;
    let revoked: string | null = null;
    let cancelled = false;
    load(mailbox, attachment.id)
      .then((bytes) => {
        if (cancelled) return;
        revoked = URL.createObjectURL(new Blob([bytes], { type: attachment.contentType }));
        setUrl(revoked);
      })
      .catch((e: unknown) => {
        log.warn("[messages] attachment failed:", errorText(e));
        if (!cancelled) setFailed(true);
      });
    return () => {
      cancelled = true;
      if (revoked) URL.revokeObjectURL(revoked);
    };
  }, [attachment.id, attachment.contentType, image, load, mailbox]);

  if (!image) {
    const kb = Math.max(1, Math.round(attachment.sizeBytes / 1024));
    return (
      <Chip
        size="small"
        icon={<AttachFileIcon />}
        label={`${attachment.contentType} · ${kb} KB`}
        sx={{ mb: 0.5, mr: 0.5 }}
      />
    );
  }
  if (failed) {
    return (
      <Typography variant="caption" sx={{ display: "block", opacity: 0.8 }}>
        {t("messages.pictureFailed")}
      </Typography>
    );
  }
  if (!url) {
    return (
      <Box sx={{ width: 160, height: 100, borderRadius: "8px", bgcolor: "rgba(0,0,0,0.1)", mb: 0.5, display: "flex", alignItems: "center", justifyContent: "center" }}>
        <CircularProgress size={18} />
      </Box>
    );
  }
  return (
    <Box component="img" src={url} alt={t("messages.picture")} sx={{ display: "block", maxWidth: "100%", maxHeight: 280, borderRadius: "8px", mb: 0.5 }} />
  );
}
