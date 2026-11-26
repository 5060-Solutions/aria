import { useEffect, useMemo, useState } from "react";
import {
  Avatar,
  Badge,
  Box,
  Button,
  Chip,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogContentText,
  DialogTitle,
  IconButton,
  List,
  ListItemAvatar,
  ListItemButton,
  ListItemText,
  TextField,
  Tooltip,
  Typography,
  alpha,
  useTheme,
} from "@mui/material";
import SmsIcon from "@mui/icons-material/Sms";
import RefreshIcon from "@mui/icons-material/Refresh";
import EditIcon from "@mui/icons-material/Edit";
import FiberManualRecordIcon from "@mui/icons-material/FiberManualRecord";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { useMessagingStore } from "../../stores/messagingStore";
import {
  formatNumber,
  isUnread,
  messagingMailboxFor,
  personLabel,
  newestConversationsFirst,
  previewOf,
  toE164,
  type Conversation,
} from "../../utils/messaging";
import { formatVoicemailTime } from "../../utils/voicemail";
import { Thread } from "./Thread";

/**
 * Business texting on the active account: the numbers the extension may use,
 * a conversation list per number with unread state, and the thread. The PBX
 * decides who may use which number; an extension with none sees why.
 */
export function Messages() {
  const { t } = useTranslation();
  const theme = useTheme();
  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const contacts = useAppStore((s) => s.contacts);
  const defaultCountry = useAppStore((s) => s.defaultCountry);
  const account = accounts.find((a) => a.id === activeAccountId);
  const mailbox = useMemo(() => messagingMailboxFor(account), [account]);

  const entry = useMessagingStore((s) => (mailbox ? s.byAccount[mailbox.accountId] : undefined));
  const readAt = useMessagingStore((s) => s.readAt);
  const refresh = useMessagingStore((s) => s.refresh);

  const [didId, setDidId] = useState<string | null>(null);
  const [number, setNumber] = useState<string | null>(null);
  const [composeOpen, setComposeOpen] = useState(false);
  const [composeText, setComposeText] = useState("");
  const [composeError, setComposeError] = useState(false);

  useEffect(() => {
    if (mailbox) void refresh(mailbox);
  }, [mailbox, refresh]);

  // A notification asked for a thread: it wins over the picks until one is made.
  const requested = useMessagingStore((s) => s.requested);
  const setRequested = useMessagingStore((s) => s.setRequested);
  const asked = requested && requested.accountId === mailbox?.accountId ? requested : null;
  const shownDid = asked?.didId ?? didId;
  const shownNumber = asked?.number ?? number;
  const pickInbox = (id: string | null) => {
    setRequested(null);
    setDidId(id);
  };
  const pickNumber = (n: string | null) => {
    setRequested(null);
    setNumber(n);
  };

  const inboxes = entry?.inboxes ?? [];
  const inbox = inboxes.find((i) => i.did_id === shownDid) ?? inboxes[0];
  const conversations = useMemo(
    () => newestConversationsFirst(inbox ? (entry?.conversations[inbox.did_id] ?? []) : []),
    [entry?.conversations, inbox],
  );

  const unreadIn = (id: string): number =>
    (entry?.conversations[id] ?? []).filter((c) =>
      isUnread(c, readAt[`${entry?.mailbox.accountId}|${id}|${c.remote_number}`], entry?.baseline ?? ""),
    ).length;

  const startNew = () => {
    const e164 = toE164(composeText, defaultCountry);
    if (!e164) {
      setComposeError(true);
      return;
    }
    setComposeOpen(false);
    setComposeText("");
    setComposeError(false);
    pickNumber(e164);
  };

  const notice = (text: string, action?: { label: string; onClick: () => void }) => (
    <Box sx={{ flex: 1, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", gap: 1.5, color: "text.secondary", px: 3, textAlign: "center" }}>
      <Typography variant="body2">{text}</Typography>
      {action && (
        <Button size="small" variant="outlined" onClick={action.onClick}>
          {action.label}
        </Button>
      )}
    </Box>
  );

  if (!mailbox || !account) {
    return <Box sx={{ height: "100%", display: "flex", flexDirection: "column" }}>{notice(t("messages.noNumbers"))}</Box>;
  }

  if (inbox && shownNumber) {
    return <Thread key={`${inbox.did_id}|${shownNumber}`} mailbox={mailbox} account={account} inbox={inbox} number={shownNumber} onBack={() => pickNumber(null)} />;
  }

  const inboxError = inbox ? entry?.inboxErrors[inbox.did_id] : undefined;
  const error = entry?.error ?? inboxError;
  const errorText =
    error?.kind === "unauthorized" || error?.kind === "noPassword"
      ? t("messages.signInFailed")
      : error
        ? `${t("messages.loadFailed")} ${error.message}`
        : "";

  const row = (c: Conversation) => {
    const unread = isUnread(c, readAt[`${mailbox.accountId}|${inbox?.did_id}|${c.remote_number}`], entry?.baseline ?? "");
    const label = personLabel(c.remote_number, contacts, defaultCountry);
    return (
      <ListItemButton key={c.remote_number} onClick={() => pickNumber(c.remote_number)} sx={{ borderRadius: "16px", mb: 0.5, py: 1 }}>
        <ListItemAvatar>
          <Avatar sx={{ width: 40, height: 40, bgcolor: alpha(theme.palette.primary.main, unread ? 0.18 : 0.08), color: "primary.main", fontSize: "0.9rem" }}>
            {label.replace(/[^\p{L}\p{N}]/gu, "").substring(0, 2).toUpperCase() || "#"}
          </Avatar>
        </ListItemAvatar>
        <ListItemText
          primary={
            <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
              {unread && <FiberManualRecordIcon sx={{ fontSize: 10, color: "primary.main" }} titleAccess={t("messages.newMessage")} />}
              <span style={{ fontWeight: unread ? 600 : 400 }}>{label}</span>
            </Box>
          }
          secondary={
            <Box component="span" sx={{ display: "flex", gap: 1, fontSize: "0.8rem", minWidth: 0 }}>
              <Box component="span" sx={{ whiteSpace: "nowrap", flexShrink: 0 }}>
                {formatVoicemailTime(c.last_at)}
              </Box>
              <Box component="span" sx={{ opacity: 0.7, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", minWidth: 0 }}>
                &middot; {c.last_direction === "outbound" ? `${t("messages.you")}: ` : ""}
                {previewOf(c.last_body, c.last_has_media, t("messages.picture"))}
              </Box>
            </Box>
          }
        />
      </ListItemButton>
    );
  };

  return (
    <Box sx={{ height: "100%", display: "flex", flexDirection: "column" }}>
      <Box sx={{ display: "flex", alignItems: "center", gap: 1, px: 2, pt: 2, pb: 1 }}>
        <SmsIcon sx={{ color: "primary.main" }} />
        <Typography variant="h6" sx={{ fontWeight: 600, flex: 1 }}>
          {t("messages.title")}
        </Typography>
        <Tooltip title={entry?.live ? t("messages.live") : t("messages.polling")}>
          <FiberManualRecordIcon sx={{ fontSize: 10, color: entry?.live ? "success.main" : "text.disabled" }} />
        </Tooltip>
        <Tooltip title={t("messages.refresh")}>
          <span>
            <IconButton size="small" onClick={() => void refresh(mailbox)} disabled={entry?.loading ?? false} aria-label={t("messages.refresh")}>
              {entry?.loading ? <CircularProgress size={18} /> : <RefreshIcon fontSize="small" />}
            </IconButton>
          </span>
        </Tooltip>
        <Button size="small" variant="contained" startIcon={<EditIcon />} disabled={!inbox?.enabled} onClick={() => setComposeOpen(true)}>
          {t("messages.newConversation")}
        </Button>
      </Box>

      {inboxes.length > 1 && (
        <Box sx={{ display: "flex", gap: 1, px: 2, pb: 1, flexWrap: "wrap" }} role="tablist" aria-label={t("messages.number")}>
          {inboxes.map((i) => (
            <Badge key={i.did_id} badgeContent={unreadIn(i.did_id)} color="error" max={99}>
              <Chip
                role="tab"
                aria-selected={i.did_id === inbox?.did_id}
                label={i.enabled ? formatNumber(i.number, defaultCountry) : t("messages.numberOff", { number: formatNumber(i.number, defaultCountry) })}
                color={i.did_id === inbox?.did_id ? "primary" : "default"}
                variant={i.did_id === inbox?.did_id ? "filled" : "outlined"}
                onClick={() => pickInbox(i.did_id)}
              />
            </Badge>
          ))}
        </Box>
      )}
      {inboxes.length === 1 && inbox && (
        <Typography variant="caption" sx={{ px: 2.5, pb: 1, color: "text.secondary" }}>
          {t("messages.from")} {formatNumber(inbox.number, defaultCountry)}
        </Typography>
      )}

      {error ? (
        notice(errorText, { label: t("messages.retry"), onClick: () => void refresh(mailbox) })
      ) : !entry?.loaded ? (
        <Box sx={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center" }}>
          <CircularProgress size={28} />
        </Box>
      ) : !inbox ? (
        notice(t("messages.noNumbers"))
      ) : conversations.length === 0 ? (
        notice(t("messages.empty"))
      ) : (
        <List sx={{ flex: 1, overflow: "auto", px: 1 }}>{conversations.map(row)}</List>
      )}

      <Dialog open={composeOpen} onClose={() => setComposeOpen(false)} fullWidth maxWidth="xs">
        <DialogTitle>{t("messages.newTitle")}</DialogTitle>
        <DialogContent>
          <DialogContentText sx={{ mb: 2 }}>{t("messages.newHelp")}</DialogContentText>
          <TextField
            autoFocus
            fullWidth
            size="small"
            label={t("messages.newNumberLabel")}
            value={composeText}
            error={composeError}
            helperText={composeError ? t("messages.invalidNumber") : undefined}
            onChange={(e) => {
              setComposeText(e.target.value);
              setComposeError(false);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") startNew();
            }}
            slotProps={{ htmlInput: { inputMode: "tel" } }}
          />
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setComposeOpen(false)}>{t("common.cancel")}</Button>
          <Button variant="contained" onClick={startNew}>
            {t("messages.start")}
          </Button>
        </DialogActions>
      </Dialog>
    </Box>
  );
}
