import { useState, useMemo } from "react";
import {
  Box,
  List,
  ListItemButton,
  ListItemAvatar,
  ListItemText,
  Avatar,
  Typography,
  IconButton,
  alpha,
  useTheme,
  Dialog,
  DialogTitle,
  DialogContent,
  DialogActions,
  Button,
  Divider,
} from "@mui/material";
import CallMadeIcon from "@mui/icons-material/CallMade";
import CallReceivedIcon from "@mui/icons-material/CallReceived";
import CallMissedIcon from "@mui/icons-material/CallMissed";
import CallIcon from "@mui/icons-material/Call";
import ArrowBackIcon from "@mui/icons-material/ArrowBack";
import AccessTimeIcon from "@mui/icons-material/AccessTime";
import TimerIcon from "@mui/icons-material/Timer";
import PhoneIcon from "@mui/icons-material/Phone";
import PlayArrowIcon from "@mui/icons-material/PlayArrow";
import FiberManualRecordIcon from "@mui/icons-material/FiberManualRecord";
import AccountCircleIcon from "@mui/icons-material/AccountCircle";
import DescriptionOutlinedIcon from "@mui/icons-material/DescriptionOutlined";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { playRecording, exportCallPcap } from "../../hooks/useSip";
import type { CallHistoryEntry } from "../../types/sip";
import { parsePhoneNumber, type CountryCode } from "libphonenumber-js";

function formatDuration(seconds: number): string {
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  if (m === 0) return `${s}s`;
  return `${m}m ${s}s`;
}

function formatDurationLong(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  if (h > 0) return `${h}h ${m}m ${s}s`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s} seconds`;
}

function formatTime(timestamp: number): string {
  const date = new Date(timestamp);
  const now = new Date();
  const isToday = date.toDateString() === now.toDateString();
  if (isToday) {
    return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }
  return date.toLocaleDateString([], { month: "short", day: "numeric" });
}

function getFlagEmoji(countryCode: string): string {
  const codePoints = countryCode
    .toUpperCase()
    .split("")
    .map((char) => 127397 + char.charCodeAt(0));
  return String.fromCodePoint(...codePoints);
}

function getNumberInfo(uri: string, defaultCountry: CountryCode) {
  const number = uri.replace(/^sip:/, "").split("@")[0];
  
  // Don't try to format short numbers (likely extensions) or non-numeric strings
  const digitsOnly = number.replace(/\D/g, "");
  if (digitsOnly.length < 7 || !/^\+?\d+$/.test(number.replace(/[\s\-()]/g, ""))) {
    return {
      formatted: number,
      country: null,
      raw: number,
    };
  }
  
  try {
    const parsed = parsePhoneNumber(number, defaultCountry);
    if (parsed && parsed.isValid()) {
      return {
        formatted: parsed.formatInternational(),
        country: parsed.country || null,
        raw: number,
      };
    }
  } catch {
    // Ignore parsing errors
  }
  return { formatted: number, country: null, raw: number };
}

function CallIcon_({ entry }: { entry: CallHistoryEntry }) {
  if (entry.missed) return <CallMissedIcon sx={{ color: "error.main", fontSize: 18 }} />;
  if (entry.direction === "outbound")
    return <CallMadeIcon sx={{ color: "primary.main", fontSize: 18 }} />;
  return <CallReceivedIcon sx={{ color: "primary.main", fontSize: 18 }} />;
}

export function CallHistory() {
  const { t, i18n } = useTranslation();
  const callHistory = useAppStore((s) => s.callHistory);
  const setDialInput = useAppStore((s) => s.setDialInput);
  const setCurrentView = useAppStore((s) => s.setCurrentView);
  const defaultCountry = useAppStore((s) => s.defaultCountry) as CountryCode;
  const theme = useTheme();
  const [selectedEntry, setSelectedEntry] = useState<CallHistoryEntry | null>(null);

  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const setActiveCall = useAppStore((s) => s.setActiveCall);
  const activeAccount = accounts.find((a) => a.id === activeAccountId);
  
  const dateTimeFormatter = useMemo(() => {
    return new Intl.DateTimeFormat(i18n.language, {
      weekday: "short",
      month: "short",
      day: "numeric",
      year: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  }, [i18n.language]);

  const handleCallBack = async (uri: string, name?: string) => {
    if (!activeAccountId || !activeAccount) {
      const number = uri.replace(/^sip:/, "").split("@")[0];
      setDialInput(number);
      setCurrentView("dialer");
      return;
    }

    const number = uri.replace(/^sip:/, "").split("@")[0];
    const numberInfo = getNumberInfo(uri, defaultCountry);
    const fullUri = uri.startsWith("sip:") ? uri : `sip:${number}@${activeAccount.domain}`;

    setActiveCall({
      id: crypto.randomUUID(),
      accountId: activeAccountId,
      remoteUri: fullUri,
      remoteName: name || numberInfo.formatted,
      state: "dialing",
      direction: "outbound",
      startTime: Date.now(),
      muted: false,
      held: false,
      recording: false,
    });

    try {
      const { sipMakeCall } = await import("../../hooks/useSip");
      const callId = await sipMakeCall(fullUri);
      setActiveCall({
        id: callId,
        accountId: activeAccountId,
        remoteUri: fullUri,
        remoteName: name || numberInfo.formatted,
        state: "dialing",
        direction: "outbound",
        startTime: Date.now(),
        muted: false,
        held: false,
        recording: false,
      });
    } catch {
      setActiveCall(null);
    }
  };

  const selectedInfo = useMemo(() => {
    if (!selectedEntry) return null;
    return getNumberInfo(selectedEntry.remoteUri, defaultCountry);
  }, [selectedEntry, defaultCountry]);

  return (
    <Box sx={{ height: "100%", display: "flex", flexDirection: "column" }}>
      <Box sx={{ px: 2.5, pt: 2.5, pb: 1 }}>
        <Typography variant="h5" sx={{ fontWeight: 500 }}>
          {t("history.title")}
        </Typography>
      </Box>

      {callHistory.length === 0 ? (
        <Box
          sx={{
            flex: 1,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: "text.secondary",
            fontSize: "0.9rem",
          }}
        >
          {t("history.noRecent")}
        </Box>
      ) : (
        <List sx={{ flex: 1, overflow: "auto", px: 1 }}>
          {callHistory.map((entry) => {
            const numberInfo = getNumberInfo(entry.remoteUri, defaultCountry);
            return (
              <ListItemButton
                key={entry.id}
                onClick={() => setSelectedEntry(entry)}
                sx={{
                  borderRadius: "16px",
                  mb: 0.5,
                  py: 1,
                }}
              >
                <ListItemAvatar>
                  <Avatar
                    sx={{
                      width: 40,
                      height: 40,
                      bgcolor: alpha(theme.palette.primary.main, 0.1),
                      color: "primary.main",
                      fontSize: numberInfo.country ? "1.2rem" : "0.9rem",
                    }}
                  >
                    {numberInfo.country
                      ? getFlagEmoji(numberInfo.country)
                      : (entry.remoteName || entry.remoteUri)
                          .replace(/^sip:/, "")
                          .substring(0, 2)
                          .toUpperCase()}
                  </Avatar>
                </ListItemAvatar>
                <ListItemText
                  primary={
                    <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
                      <CallIcon_ entry={entry} />
                      <span style={{ color: entry.missed ? theme.palette.error.main : undefined }}>
                        {entry.remoteName || numberInfo.formatted}
                      </span>
                      {entry.recordingPath && (
                        <FiberManualRecordIcon sx={{ fontSize: 10, color: "error.main", ml: 0.5 }} />
                      )}
                    </Box>
                  }
                  secondary={
                    <Box
                      component="span"
                      sx={{ display: "flex", gap: 1, fontSize: "0.8rem" }}
                    >
                      <span>{formatTime(entry.startTime)}</span>
                      {!entry.missed && (
                        <span>
                          &middot; {formatDuration(entry.duration)}
                        </span>
                      )}
                      {entry.missed && (
                        <span style={{ color: theme.palette.error.main }}>
                          &middot; {t("history.missed")}
                        </span>
                      )}
                      {accounts.length > 1 && entry.accountId && (
                        <span style={{ opacity: 0.7 }}>
                          &middot; {accounts.find(a => a.id === entry.accountId)?.displayName || "—"}
                        </span>
                      )}
                    </Box>
                  }
                  primaryTypographyProps={{ fontSize: "0.9rem", fontWeight: 500 }}
                />
                <IconButton
                  size="small"
                  onClick={(e) => {
                    e.stopPropagation();
                    handleCallBack(entry.remoteUri, entry.remoteName);
                  }}
                  sx={{
                    color: "primary.main",
                    borderRadius: "12px",
                    bgcolor: alpha(theme.palette.primary.main, 0.08),
                    "&:hover": {
                      bgcolor: alpha(theme.palette.primary.main, 0.16),
                    },
                  }}
                >
                  <CallIcon fontSize="small" />
                </IconButton>
              </ListItemButton>
            );
          })}
        </List>
      )}

      {/* Call Detail Dialog */}
      <Dialog
        open={Boolean(selectedEntry)}
        onClose={() => setSelectedEntry(null)}
        maxWidth="xs"
        fullWidth
        PaperProps={{
          sx: {
            borderRadius: "20px",
            bgcolor: "background.paper",
          },
        }}
      >
        {selectedEntry && selectedInfo && (
          <>
            <DialogTitle sx={{ pb: 1 }}>
              <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
                <IconButton
                  size="small"
                  onClick={() => setSelectedEntry(null)}
                  sx={{ mr: 1 }}
                >
                  <ArrowBackIcon fontSize="small" />
                </IconButton>
                {t("history.callDetails")}
              </Box>
            </DialogTitle>
            <DialogContent>
              <Box sx={{ textAlign: "center", py: 2 }}>
                <Avatar
                  sx={{
                    width: 72,
                    height: 72,
                    mx: "auto",
                    mb: 2,
                    bgcolor: alpha(theme.palette.primary.main, 0.1),
                    color: "primary.main",
                    fontSize: selectedInfo.country ? "2.5rem" : "1.5rem",
                  }}
                >
                  {selectedInfo.country
                    ? getFlagEmoji(selectedInfo.country)
                    : selectedInfo.raw.substring(0, 2).toUpperCase()}
                </Avatar>
                <Typography variant="h6" sx={{ fontWeight: 600 }}>
                  {selectedEntry.remoteName || selectedInfo.formatted}
                </Typography>
                {selectedEntry.remoteName && (
                  <Typography variant="body2" sx={{ color: "text.secondary" }}>
                    {selectedInfo.formatted}
                  </Typography>
                )}
              </Box>

              <Divider sx={{ my: 2 }} />

              <Box sx={{ display: "flex", flexDirection: "column", gap: 2 }}>
                <Box sx={{ display: "flex", alignItems: "center", gap: 2 }}>
                  {selectedEntry.direction === "outbound" ? (
                    <CallMadeIcon sx={{ color: "text.secondary" }} />
                  ) : (
                    <CallReceivedIcon sx={{ color: "text.secondary" }} />
                  )}
                  <Box>
                    <Typography variant="body2" sx={{ fontWeight: 500 }}>
                      {selectedEntry.direction === "outbound" ? t("history.outgoing") : t("history.incomingCall")}
                      {selectedEntry.missed && ` ${t("history.missed")}`}
                    </Typography>
                    <Typography variant="caption" sx={{ color: "text.secondary" }}>
                      {t("history.direction")}
                    </Typography>
                  </Box>
                </Box>

                <Box sx={{ display: "flex", alignItems: "center", gap: 2 }}>
                  <AccessTimeIcon sx={{ color: "text.secondary" }} />
                  <Box>
                    <Typography variant="body2" sx={{ fontWeight: 500 }}>
                      {dateTimeFormatter.format(selectedEntry.startTime)}
                    </Typography>
                    <Typography variant="caption" sx={{ color: "text.secondary" }}>
                      {t("history.dateTime")}
                    </Typography>
                  </Box>
                </Box>

                {!selectedEntry.missed && (
                  <Box sx={{ display: "flex", alignItems: "center", gap: 2 }}>
                    <TimerIcon sx={{ color: "text.secondary" }} />
                    <Box>
                      <Typography variant="body2" sx={{ fontWeight: 500 }}>
                        {formatDurationLong(selectedEntry.duration)}
                      </Typography>
                      <Typography variant="caption" sx={{ color: "text.secondary" }}>
                        {t("history.duration")}
                      </Typography>
                    </Box>
                  </Box>
                )}

                <Box sx={{ display: "flex", alignItems: "center", gap: 2 }}>
                  <PhoneIcon sx={{ color: "text.secondary" }} />
                  <Box>
                    <Typography variant="body2" sx={{ fontWeight: 500, fontFamily: "monospace" }}>
                      {selectedEntry.remoteUri}
                    </Typography>
                    <Typography variant="caption" sx={{ color: "text.secondary" }}>
                      {t("history.sipUri")}
                    </Typography>
                  </Box>
                </Box>

                {selectedEntry.accountId && (
                  <Box sx={{ display: "flex", alignItems: "center", gap: 2 }}>
                    <AccountCircleIcon sx={{ color: "text.secondary" }} />
                    <Box>
                      <Typography variant="body2" sx={{ fontWeight: 500 }}>
                        {accounts.find(a => a.id === selectedEntry.accountId)?.displayName || selectedEntry.accountId}
                      </Typography>
                      <Typography variant="caption" sx={{ color: "text.secondary" }}>
                        {t("history.account")}
                      </Typography>
                    </Box>
                  </Box>
                )}

                {selectedEntry.recordingPath && (
                  <Box sx={{ display: "flex", alignItems: "center", gap: 2 }}>
                    <FiberManualRecordIcon sx={{ color: "error.main" }} />
                    <Box sx={{ flex: 1 }}>
                      <Typography variant="body2" sx={{ fontWeight: 500 }}>
                        {t("history.callRecorded")}
                      </Typography>
                      <Typography variant="caption" sx={{ color: "text.secondary" }}>
                        {t("history.recordingAvailable")}
                      </Typography>
                    </Box>
                    <IconButton
                      size="small"
                      onClick={() => playRecording(selectedEntry.recordingPath ?? "")}
                      sx={{
                        bgcolor: alpha(theme.palette.primary.main, 0.1),
                        color: "primary.main",
                        "&:hover": {
                          bgcolor: alpha(theme.palette.primary.main, 0.2),
                        },
                      }}
                    >
                      <PlayArrowIcon />
                    </IconButton>
                  </Box>
                )}

                {selectedEntry.sipCallId && (
                  <Box sx={{ display: "flex", alignItems: "center", gap: 2 }}>
                    <DescriptionOutlinedIcon sx={{ color: "text.secondary" }} />
                    <Box sx={{ flex: 1 }}>
                      <Typography variant="body2" sx={{ fontWeight: 500 }}>
                        {t("history.sipTrace")}
                      </Typography>
                      <Typography variant="caption" sx={{ color: "text.secondary" }}>
                        {t("history.sipTraceDescription")}
                      </Typography>
                    </Box>
                    <Button
                      size="small"
                      variant="outlined"
                      onClick={async () => {
                        try {
                          const { save } = await import("@tauri-apps/plugin-dialog");
                          const sipId = selectedEntry.sipCallId;
                          if (!sipId) return;
                          const path = await save({
                            defaultPath: `aria-call-${selectedEntry.id.substring(0, 8)}.pcap`,
                            filters: [{ name: "PCAP Files", extensions: ["pcap"] }],
                          });
                          if (path) {
                            await exportCallPcap(sipId, path);
                          }
                        } catch {
                          // Export failed silently
                        }
                      }}
                      sx={{
                        borderRadius: "8px",
                        fontSize: "0.7rem",
                        minWidth: "auto",
                        px: 1.5,
                      }}
                    >
                      PCAP
                    </Button>
                  </Box>
                )}
              </Box>
            </DialogContent>
            <DialogActions sx={{ px: 3, pb: 3 }}>
              <Button
                fullWidth
                variant="contained"
                startIcon={<CallIcon />}
                onClick={() => {
                  handleCallBack(selectedEntry.remoteUri, selectedEntry.remoteName);
                  setSelectedEntry(null);
                }}
                sx={{ borderRadius: "12px", py: 1.5 }}
              >
                {t("history.callBack")}
              </Button>
            </DialogActions>
          </>
        )}
      </Dialog>
    </Box>
  );
}
