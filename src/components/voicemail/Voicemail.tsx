import { useEffect, useMemo, useRef, useState } from "react";
import {
  Avatar,
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
  Tooltip,
  Typography,
  alpha,
  useTheme,
} from "@mui/material";
import VoicemailIcon from "@mui/icons-material/Voicemail";
import RefreshIcon from "@mui/icons-material/Refresh";
import ArrowBackIcon from "@mui/icons-material/ArrowBack";
import PhoneIcon from "@mui/icons-material/Phone";
import DeleteIcon from "@mui/icons-material/Delete";
import FiberManualRecordIcon from "@mui/icons-material/FiberManualRecord";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { useVoicemailStore } from "../../stores/voicemailStore";
import { errorText, notifyError } from "../../stores/toastStore";
import { placeCall } from "../../hooks/useSip";
import { dialUri } from "../../utils/sipUri";
import { log } from "../../utils/log";
import {
  callerLabel,
  formatVoicemailDuration,
  formatVoicemailTime,
  mailboxFor,
  sortNewestFirst,
  unreadCount,
  type VoicemailError,
  type VoicemailMessage,
} from "../../utils/voicemail";

interface AudioState {
  messageId: string;
  url: string | null;
  loading: boolean;
  failed: boolean;
}

/**
 * The active account's mailbox on an Aria PBX: who called, when, how long,
 * the transcription when the PBX made one, and the message itself played in
 * place. Opening a message marks it read, as a desk phone would.
 */
export function Voicemail() {
  const { t } = useTranslation();
  const theme = useTheme();
  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const contacts = useAppStore((s) => s.contacts);
  const defaultCountry = useAppStore((s) => s.defaultCountry);
  const account = accounts.find((a) => a.id === activeAccountId);
  const mailbox = useMemo(() => mailboxFor(account), [account]);

  const entry = useVoicemailStore((s) => (mailbox ? s.byAccount[mailbox.accountId] : undefined));
  const refresh = useVoicemailStore((s) => s.refresh);
  const markRead = useVoicemailStore((s) => s.markRead);
  const remove = useVoicemailStore((s) => s.remove);
  const loadAudio = useVoicemailStore((s) => s.loadAudio);

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [audio, setAudio] = useState<AudioState | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleting, setDeleting] = useState(false);
  // The blob URL in play, revoked when another message opens or the view
  // closes; a ref because the unmount cleanup must see the latest one.
  const audioUrl = useRef<string | null>(null);

  useEffect(() => {
    if (mailbox) void refresh(mailbox);
  }, [mailbox, refresh]);

  useEffect(() => {
    return () => {
      if (audioUrl.current) URL.revokeObjectURL(audioUrl.current);
    };
  }, []);

  const messages = useMemo(() => sortNewestFirst(entry?.messages ?? []), [entry?.messages]);
  const selected = messages.find((m) => m.id === selectedId) ?? null;
  const unread = unreadCount(messages);

  const releaseAudio = () => {
    if (audioUrl.current) URL.revokeObjectURL(audioUrl.current);
    audioUrl.current = null;
  };

  const open = async (message: VoicemailMessage) => {
    if (!mailbox) return;
    releaseAudio();
    setSelectedId(message.id);
    setAudio({ messageId: message.id, url: null, loading: true, failed: false });
    if (!message.read) {
      markRead(mailbox, message.id).catch((e) => log.warn("[voicemail] mark read failed:", errorText(e)));
    }
    try {
      const bytes = await loadAudio(mailbox, message.id);
      const url = URL.createObjectURL(new Blob([bytes], { type: "audio/wav" }));
      audioUrl.current = url;
      setAudio({ messageId: message.id, url, loading: false, failed: false });
    } catch (e) {
      log.error("[voicemail] audio failed:", errorText(e));
      setAudio({ messageId: message.id, url: null, loading: false, failed: true });
    }
  };

  const back = () => {
    releaseAudio();
    setAudio(null);
    setSelectedId(null);
  };

  const callBack = (message: VoicemailMessage) => {
    if (!account) return;
    const label = callerLabel(message.callerId, contacts);
    placeCall({
      uri: dialUri(message.callerId, account, defaultCountry),
      accountId: account.id,
      remoteName: label ?? undefined,
    }).catch((e) => notifyError(errorText(e)));
  };

  const deleteSelected = async () => {
    if (!mailbox || !selected) return;
    setDeleting(true);
    try {
      await remove(mailbox, selected.id);
      setConfirmDelete(false);
      back();
    } catch (e) {
      notifyError(errorText(e));
    } finally {
      setDeleting(false);
    }
  };

  const header = (
    <Box sx={{ display: "flex", alignItems: "center", gap: 1, px: 2, pt: 2, pb: 1 }}>
      {selected ? (
        <IconButton size="small" onClick={back} aria-label={t("voicemail.back")}>
          <ArrowBackIcon fontSize="small" />
        </IconButton>
      ) : (
        <VoicemailIcon sx={{ color: "primary.main" }} />
      )}
      <Typography variant="h6" sx={{ fontWeight: 600, flex: 1 }}>
        {t("voicemail.title")}
      </Typography>
      {!selected && unread > 0 && (
        <Chip size="small" color="primary" label={t("voicemail.unread", { count: unread })} />
      )}
      {!selected && mailbox && (
        <Tooltip title={t("voicemail.refresh")}>
          <span>
            <IconButton
              size="small"
              onClick={() => void refresh(mailbox)}
              disabled={entry?.loading ?? false}
              aria-label={t("voicemail.refresh")}
            >
              {entry?.loading ? <CircularProgress size={18} /> : <RefreshIcon fontSize="small" />}
            </IconButton>
          </span>
        </Tooltip>
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

  const errorNotice = (error: VoicemailError) => {
    const text =
      error.kind === "noMailbox"
        ? t("voicemail.noMailbox")
        : error.kind === "unauthorized" || error.kind === "noPassword"
          ? t("voicemail.signInFailed")
          : `${t("voicemail.loadFailed")} ${error.message}`;
    const retry = error.kind === "noMailbox" || !mailbox ? undefined : { label: t("voicemail.retry"), onClick: () => void refresh(mailbox) };
    return notice(text, retry);
  };

  if (!mailbox || !account) {
    // Only reachable if the active account changed under an open view.
    return (
      <Box sx={{ height: "100%", display: "flex", flexDirection: "column" }}>
        {header}
        {notice(t("voicemail.noMailbox"))}
      </Box>
    );
  }

  if (selected) {
    const label = callerLabel(selected.callerId, contacts);
    return (
      <Box sx={{ height: "100%", display: "flex", flexDirection: "column" }}>
        {header}
        <Box sx={{ px: 3, py: 2, display: "flex", flexDirection: "column", gap: 2, overflow: "auto" }}>
          <Box sx={{ display: "flex", alignItems: "center", gap: 2 }}>
            <Avatar sx={{ width: 48, height: 48, bgcolor: alpha(theme.palette.primary.main, 0.1), color: "primary.main" }}>
              {(label ?? "?").substring(0, 2).toUpperCase()}
            </Avatar>
            <Box sx={{ flex: 1, minWidth: 0 }}>
              <Typography variant="subtitle1" sx={{ fontWeight: 600 }} noWrap>
                {label ?? t("voicemail.unknownCaller")}
              </Typography>
              <Typography variant="body2" sx={{ color: "text.secondary" }}>
                {formatVoicemailTime(selected.createdAt)} · {formatVoicemailDuration(selected.durationSecs)}
              </Typography>
            </Box>
          </Box>

          {audio?.loading && (
            <Box sx={{ display: "flex", alignItems: "center", gap: 1, color: "text.secondary" }}>
              <CircularProgress size={16} />
              <Typography variant="body2">{t("voicemail.loadingAudio")}</Typography>
            </Box>
          )}
          {audio?.failed && (
            <Typography variant="body2" sx={{ color: "error.main" }}>
              {t("voicemail.audioFailed")}
            </Typography>
          )}
          {audio?.url && (
            // Native controls: play, seek, volume, and the OS media keys.
            <audio controls src={audio.url} style={{ width: "100%" }} data-testid="voicemail-player" />
          )}

          <Box>
            <Typography variant="overline" sx={{ color: "text.secondary" }}>
              {t("voicemail.transcription")}
            </Typography>
            <Typography variant="body2" sx={{ whiteSpace: "pre-wrap", color: selected.transcription ? "text.primary" : "text.disabled" }}>
              {selected.transcription?.trim() || t("voicemail.noTranscription")}
            </Typography>
          </Box>

          <Box sx={{ display: "flex", gap: 1, flexWrap: "wrap" }}>
            {label && (
              <Button variant="contained" size="small" startIcon={<PhoneIcon />} onClick={() => callBack(selected)}>
                {t("voicemail.callBack")}
              </Button>
            )}
            <Button
              variant="outlined"
              color="error"
              size="small"
              startIcon={<DeleteIcon />}
              onClick={() => setConfirmDelete(true)}
            >
              {t("voicemail.delete")}
            </Button>
          </Box>
        </Box>

        <Dialog open={confirmDelete} onClose={() => setConfirmDelete(false)}>
          <DialogTitle>{t("voicemail.deleteTitle")}</DialogTitle>
          <DialogContent>
            <DialogContentText>{t("voicemail.deleteBody")}</DialogContentText>
          </DialogContent>
          <DialogActions>
            <Button onClick={() => setConfirmDelete(false)} disabled={deleting}>
              {t("common.cancel")}
            </Button>
            <Button color="error" variant="contained" onClick={() => void deleteSelected()} disabled={deleting}>
              {t("voicemail.delete")}
            </Button>
          </DialogActions>
        </Dialog>
      </Box>
    );
  }

  return (
    <Box sx={{ height: "100%", display: "flex", flexDirection: "column" }}>
      {header}
      {entry?.error ? (
        errorNotice(entry.error)
      ) : !entry?.loaded ? (
        <Box sx={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center" }}>
          <CircularProgress size={28} />
        </Box>
      ) : messages.length === 0 ? (
        notice(t("voicemail.empty"))
      ) : (
        <List sx={{ flex: 1, overflow: "auto", px: 1 }}>
          {messages.map((message) => {
            const label = callerLabel(message.callerId, contacts);
            return (
              <ListItemButton
                key={message.id}
                onClick={() => void open(message)}
                sx={{ borderRadius: "16px", mb: 0.5, py: 1 }}
              >
                <ListItemAvatar>
                  <Avatar
                    sx={{
                      width: 40,
                      height: 40,
                      bgcolor: alpha(theme.palette.primary.main, message.read ? 0.08 : 0.18),
                      color: "primary.main",
                      fontSize: "0.9rem",
                    }}
                  >
                    {(label ?? "?").substring(0, 2).toUpperCase()}
                  </Avatar>
                </ListItemAvatar>
                <ListItemText
                  primary={
                    <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
                      {!message.read && (
                        <FiberManualRecordIcon sx={{ fontSize: 10, color: "primary.main" }} titleAccess={t("voicemail.newMessage")} />
                      )}
                      <span style={{ fontWeight: message.read ? 400 : 600 }}>
                        {label ?? t("voicemail.unknownCaller")}
                      </span>
                    </Box>
                  }
                  secondary={
                    <Box component="span" sx={{ display: "flex", gap: 1, fontSize: "0.8rem" }}>
                      <span>{formatVoicemailTime(message.createdAt)}</span>
                      <span>&middot; {formatVoicemailDuration(message.durationSecs)}</span>
                      {message.transcription && (
                        <Box component="span" sx={{ opacity: 0.7, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                          &middot; {message.transcription}
                        </Box>
                      )}
                    </Box>
                  }
                />
              </ListItemButton>
            );
          })}
        </List>
      )}
    </Box>
  );
}
