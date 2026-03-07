import { useEffect, useState, useReducer, useCallback, useRef, useMemo } from "react";
import {
  Box,
  Typography,
  IconButton,
  Chip,
  alpha,
  useTheme,
  Tooltip,
  Divider,
  LinearProgress,
  Tabs,
  Tab,
  Menu,
  MenuItem,
  ListItemIcon,
  ListItemText,
} from "@mui/material";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutline";
import ArrowBackIcon from "@mui/icons-material/ArrowBack";
import FiberManualRecordIcon from "@mui/icons-material/FiberManualRecord";
import SignalCellularAltIcon from "@mui/icons-material/SignalCellularAlt";
import PhoneInTalkIcon from "@mui/icons-material/PhoneInTalk";
import SwapVertIcon from "@mui/icons-material/SwapVert";
import TimerOutlinedIcon from "@mui/icons-material/TimerOutlined";
import FileDownloadOutlinedIcon from "@mui/icons-material/FileDownloadOutlined";
import DescriptionOutlinedIcon from "@mui/icons-material/DescriptionOutlined";
import HistoryIcon from "@mui/icons-material/History";
import AssessmentOutlinedIcon from "@mui/icons-material/AssessmentOutlined";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { log } from "../../utils/log";

interface DiagnosticLog {
  timestamp: number;
  accountId: string;
  direction: "sent" | "received";
  remoteAddr: string;
  summary: string;
  raw: string;
}

interface RtpStats {
  packetsSent: number;
  packetsReceived: number;
  bytesSent: number;
  bytesReceived: number;
  packetsLost: number;
  jitterMs: number;
  codecName: string;
}

interface CallStatusInfo {
  id: string;
  accountId: string;
  remoteUri: string;
  state: string;
  direction: string;
  durationSecs: number | null;
  codec: string | null;
  rtpStats: RtpStats | null;
}

interface AccountStatus {
  accountId: string;
  username: string;
  domain: string;
  registrationState: string;
  registrationError: string | null;
  serverAddress: string | null;
  transportType: string | null;
  localAddress: string | null;
  publicAddress: string | null;
  uptimeSecs: number | null;
  activeCalls: CallStatusInfo[];
}

interface SystemStatus {
  accounts: AccountStatus[];
  latencyMs: number | null;
  totalActiveCalls: number;
}

// --- Subcomponents ---

function StatusDot({ color }: { color: string }) {
  return (
    <FiberManualRecordIcon
      sx={{
        fontSize: 10,
        color,
        filter: `drop-shadow(0 0 3px ${color})`,
      }}
    />
  );
}

function StatRow({
  label,
  value,
  mono,
}: {
  label: string;
  value: string | number | null | undefined;
  mono?: boolean;
}) {
  const theme = useTheme();
  return (
    <Box
      sx={{
        display: "flex",
        justifyContent: "space-between",
        alignItems: "center",
        py: 0.3,
      }}
    >
      <Typography
        variant="caption"
        sx={{ color: "text.secondary", fontSize: "0.7rem" }}
      >
        {label}
      </Typography>
      <Typography
        variant="caption"
        sx={{
          fontFamily: mono ? "monospace" : undefined,
          fontSize: "0.7rem",
          color: theme.palette.text.primary,
          fontWeight: 500,
        }}
      >
        {value ?? "-"}
      </Typography>
    </Box>
  );
}

function SectionCard({
  children,
  sx,
}: {
  children: React.ReactNode;
  sx?: object;
}) {
  const theme = useTheme();
  return (
    <Box
      sx={{
        mx: 1.5,
        mb: 1,
        p: 1.5,
        borderRadius: "12px",
        bgcolor: alpha(theme.palette.background.paper, 0.6),
        border: `1px solid ${alpha(theme.palette.divider, 0.08)}`,
        ...sx,
      }}
    >
      {children}
    </Box>
  );
}

function SectionHeader({
  icon,
  title,
  trailing,
}: {
  icon: React.ReactNode;
  title: string;
  trailing?: React.ReactNode;
}) {
  return (
    <Box
      sx={{
        display: "flex",
        alignItems: "center",
        gap: 0.75,
        mb: 0.75,
      }}
    >
      {icon}
      <Typography
        variant="caption"
        sx={{ fontWeight: 600, fontSize: "0.72rem", flex: 1 }}
      >
        {title}
      </Typography>
      {trailing}
    </Box>
  );
}

function registrationColor(state: string): string {
  switch (state) {
    case "registered":
      return "#4caf50";
    case "registering":
      return "#ff9800";
    case "error":
      return "#f44336";
    default:
      return "#9e9e9e";
  }
}

function useRegistrationLabel() {
  const { t } = useTranslation();
  return useCallback((state: string): string => {
    switch (state) {
      case "registered":
        return t("diagnostics.registered");
      case "registering":
        return t("diagnostics.registering");
      case "error":
        return t("diagnostics.error");
      default:
        return t("diagnostics.unregistered");
    }
  }, [t]);
}

function formatUptime(secs: number | null | undefined): string {
  if (secs == null) return "-";
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

function formatDuration(secs: number | null | undefined): string {
  if (secs == null) return "0:00";
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function latencyColor(ms: number | null | undefined): string {
  if (ms == null) return "#9e9e9e";
  if (ms < 100) return "#4caf50";
  if (ms < 250) return "#ff9800";
  return "#f44336";
}

function lossPercent(stats: RtpStats): string {
  const total = stats.packetsReceived + stats.packetsLost;
  if (total === 0) return "0%";
  return `${((stats.packetsLost / total) * 100).toFixed(1)}%`;
}

// --- Connection Card ---

function ConnectionCard({ account, latencyMs }: { account: AccountStatus; latencyMs: number | null }) {
  const { t } = useTranslation();
  const theme = useTheme();
  const registrationLabel = useRegistrationLabel();
  const state = account.registrationState ?? "unregistered";
  const color = registrationColor(state);

  return (
    <SectionCard>
      <SectionHeader
        icon={<SignalCellularAltIcon sx={{ fontSize: 14, color }} />}
        title={t("diagnostics.connection")}
        trailing={
          <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
            <StatusDot color={color} />
            <Typography
              variant="caption"
              sx={{ fontSize: "0.65rem", color, fontWeight: 600 }}
            >
              {registrationLabel(state)}
            </Typography>
          </Box>
        }
      />

      {account.registrationError && (
        <Typography
          variant="caption"
          sx={{
            color: "error.main",
            fontSize: "0.65rem",
            display: "block",
            mb: 0.5,
            px: 0.5,
            py: 0.25,
            borderRadius: "6px",
            bgcolor: alpha(theme.palette.error.main, 0.08),
          }}
        >
          {account.registrationError}
        </Typography>
      )}

      <StatRow
        label={t("diagnostics.server")}
        value={
          account.domain
            ? `${account.domain}${account.serverAddress ? ` (${account.serverAddress})` : ""}`
            : account.serverAddress
        }
        mono
      />
      <StatRow
        label={t("diagnostics.transport")}
        value={account.transportType?.toUpperCase()}
      />
      <StatRow label={t("diagnostics.local")} value={account.localAddress} mono />
      <StatRow label={t("diagnostics.publicNat")} value={account.publicAddress} mono />
      <StatRow
        label={t("diagnostics.identity")}
        value={
          account.username && account.domain
            ? `${account.username}@${account.domain}`
            : undefined
        }
        mono
      />

      <Divider sx={{ my: 0.75, opacity: 0.3 }} />

      <Box
        sx={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
        }}
      >
        <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
          <TimerOutlinedIcon sx={{ fontSize: 12, color: "text.secondary" }} />
          <Typography
            variant="caption"
            sx={{ fontSize: "0.65rem", color: "text.secondary" }}
          >
            {t("diagnostics.uptime")}: {formatUptime(account.uptimeSecs)}
          </Typography>
        </Box>
        <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
          <SwapVertIcon
            sx={{
              fontSize: 12,
              color: latencyColor(latencyMs),
            }}
          />
          <Typography
            variant="caption"
            sx={{
              fontSize: "0.65rem",
              fontFamily: "monospace",
              color: latencyColor(latencyMs),
              fontWeight: 600,
            }}
          >
            {latencyMs != null
              ? `${latencyMs.toFixed(0)}ms`
              : "-"}
          </Typography>
        </Box>
      </Box>
    </SectionCard>
  );
}

// --- Active Call Card ---

function ActiveCallCard({ call }: { call: CallStatusInfo }) {
  const { t } = useTranslation();
  const theme = useTheme();
  const stats = call.rtpStats;
  const hasStats = stats != null;

  return (
    <SectionCard>
      <SectionHeader
        icon={
          <PhoneInTalkIcon sx={{ fontSize: 14, color: "success.main" }} />
        }
        title={t("diagnostics.activeCall")}
        trailing={
          <Chip
            label={call.state}
            size="small"
            sx={{
              height: 18,
              fontSize: "0.6rem",
              fontWeight: 700,
              bgcolor: alpha(theme.palette.success.main, 0.12),
              color: "success.main",
            }}
          />
        }
      />

      <StatRow label={t("diagnostics.remote")} value={call.remoteUri} mono />
      <StatRow
        label={t("diagnostics.direction")}
        value={call.direction === "inbound" ? t("diagnostics.inbound") : t("diagnostics.outbound")}
      />
      <StatRow label={t("diagnostics.duration")} value={formatDuration(call.durationSecs)} />
      <StatRow label={t("diagnostics.codec")} value={call.codec ?? "-"} />

      {hasStats && (
        <>
          <Divider sx={{ my: 0.75, opacity: 0.3 }} />
          <Box
            sx={{
              display: "grid",
              gridTemplateColumns: "1fr 1fr",
              gap: 0.3,
            }}
          >
            <StatRow
              label={t("diagnostics.txPackets")}
              value={stats.packetsSent.toLocaleString()}
              mono
            />
            <StatRow
              label={t("diagnostics.rxPackets")}
              value={stats.packetsReceived.toLocaleString()}
              mono
            />
            <StatRow label={t("diagnostics.txBytes")} value={formatBytes(stats.bytesSent)} mono />
            <StatRow
              label={t("diagnostics.rxBytes")}
              value={formatBytes(stats.bytesReceived)}
              mono
            />
          </Box>
          <Divider sx={{ my: 0.5, opacity: 0.2 }} />
          <Box
            sx={{
              display: "flex",
              justifyContent: "space-around",
              mt: 0.25,
            }}
          >
            <Box sx={{ textAlign: "center" }}>
              <Typography
                variant="caption"
                sx={{
                  fontSize: "0.9rem",
                  fontWeight: 700,
                  fontFamily: "monospace",
                  color:
                    stats.packetsLost > 0
                      ? "warning.main"
                      : "text.primary",
                }}
              >
                {lossPercent(stats)}
              </Typography>
              <Typography
                variant="caption"
                sx={{
                  fontSize: "0.55rem",
                  color: "text.secondary",
                  display: "block",
                }}
              >
                {t("diagnostics.packetLoss")}
              </Typography>
            </Box>
            <Box sx={{ textAlign: "center" }}>
              <Typography
                variant="caption"
                sx={{
                  fontSize: "0.9rem",
                  fontWeight: 700,
                  fontFamily: "monospace",
                  color:
                    stats.jitterMs > 30
                      ? "warning.main"
                      : "text.primary",
                }}
              >
                {stats.jitterMs.toFixed(1)}
              </Typography>
              <Typography
                variant="caption"
                sx={{
                  fontSize: "0.55rem",
                  color: "text.secondary",
                  display: "block",
                }}
              >
                {t("diagnostics.jitterMs")}
              </Typography>
            </Box>
          </Box>
        </>
      )}
    </SectionCard>
  );
}

// --- SIP Log Entry ---

function SipLogEntry({
  log,
  isSelected,
  onToggle,
}: {
  log: DiagnosticLog;
  isSelected: boolean;
  onToggle: () => void;
}) {
  const { t, i18n } = useTranslation();
  const theme = useTheme();
  const isSent = log.direction === "sent";
  
  const logTimeFormatter = useMemo(() => new Intl.DateTimeFormat(i18n.language, {
    hour12: false,
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }), [i18n.language]);

  const summaryLine = log.summary;
  const isError =
    summaryLine.includes("4") ||
    summaryLine.includes("5") ||
    summaryLine.includes("6");
  const isSuccess =
    summaryLine.startsWith("SIP/2.0 200") ||
    summaryLine.startsWith("SIP/2.0 202");
  const isAuth =
    summaryLine.includes("401") || summaryLine.includes("407");
  const isRinging =
    summaryLine.includes("180") || summaryLine.includes("183");

  const methodColor = isError
    ? theme.palette.error.main
    : isSuccess
      ? theme.palette.success.main
      : isAuth
        ? theme.palette.warning.main
        : isRinging
          ? theme.palette.info.main
          : theme.palette.text.primary;

  return (
    <Box
      onClick={onToggle}
      sx={{
        px: 1,
        py: 0.6,
        mb: 0.25,
        borderRadius: "8px",
        cursor: "pointer",
        borderLeft: `2px solid ${isSent ? alpha(theme.palette.info.main, 0.5) : alpha(theme.palette.success.main, 0.5)}`,
        bgcolor: isSelected
          ? alpha(theme.palette.primary.main, 0.06)
          : "transparent",
        "&:hover": {
          bgcolor: alpha(theme.palette.text.primary, 0.03),
        },
        transition: "background-color 0.15s",
      }}
    >
      <Box
        sx={{
          display: "flex",
          alignItems: "center",
          gap: 0.5,
        }}
      >
          <Chip
            label={isSent ? t("diagnostics.tx") : t("diagnostics.rx")}
            size="small"
            sx={{
            height: 16,
            fontSize: "0.55rem",
            fontWeight: 700,
            minWidth: 28,
            bgcolor: isSent
              ? alpha(theme.palette.info.main, 0.1)
              : alpha(theme.palette.success.main, 0.1),
            color: isSent ? "info.main" : "success.main",
            "& .MuiChip-label": { px: 0.5 },
          }}
        />
        <Typography
          variant="caption"
          sx={{
            fontFamily: "monospace",
            fontSize: "0.6rem",
            color: "text.secondary",
            opacity: 0.7,
          }}
        >
          {logTimeFormatter.format(new Date(log.timestamp))}
        </Typography>
        <Typography
          variant="caption"
          sx={{
            fontFamily: "monospace",
            fontSize: "0.58rem",
            color: "text.secondary",
            ml: "auto",
            opacity: 0.5,
          }}
        >
          {log.remoteAddr}
        </Typography>
      </Box>
      <Typography
        variant="body2"
        sx={{
          fontSize: "0.7rem",
          fontFamily: "monospace",
          mt: 0.2,
          whiteSpace: "nowrap",
          overflow: "hidden",
          textOverflow: "ellipsis",
          color: methodColor,
          fontWeight: 500,
        }}
      >
        {summaryLine}
      </Typography>

      {isSelected && (
        <Box
          sx={{
            mt: 0.75,
            p: 1,
            borderRadius: "6px",
            bgcolor: alpha(theme.palette.text.primary, 0.03),
            maxHeight: 220,
            overflow: "auto",
            "&::-webkit-scrollbar": { width: 3 },
            "&::-webkit-scrollbar-thumb": {
              bgcolor: alpha(theme.palette.text.primary, 0.1),
              borderRadius: 2,
            },
          }}
        >
          <Typography
            component="pre"
            sx={{
              fontSize: "0.58rem",
              fontFamily: "monospace",
              whiteSpace: "pre-wrap",
              wordBreak: "break-all",
              m: 0,
              color: "text.secondary",
              lineHeight: 1.5,
            }}
          >
            {log.raw}
          </Typography>
        </Box>
      )}
    </Box>
  );
}

// --- Account Tab Content ---

function AccountTabContent({ 
  account, 
  logs, 
  latencyMs,
  selectedIdx,
  setSelectedIdx,
}: { 
  account: AccountStatus; 
  logs: DiagnosticLog[];
  latencyMs: number | null;
  selectedIdx: number | null;
  setSelectedIdx: (idx: number | null) => void;
}) {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = scrollRef.current;
    if (el) {
      el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
    }
  }, [logs.length]);

  return (
    <>
      <ConnectionCard account={account} latencyMs={latencyMs} />

      {account.activeCalls.map((call) => (
        <ActiveCallCard key={call.id} call={call} />
      ))}

      <Box sx={{ px: 1.5, pt: 0.5, pb: 0.25 }}>
        <Typography
          variant="caption"
          sx={{
            fontWeight: 600,
            fontSize: "0.68rem",
            color: "text.secondary",
            textTransform: "uppercase",
            letterSpacing: "0.05em",
          }}
        >
          {t("diagnostics.sipMessages", { count: logs.length })}
        </Typography>
      </Box>
      <Box ref={scrollRef} sx={{ px: 0.5 }}>
        {logs.length === 0 && (
          <Box
            sx={{
              textAlign: "center",
              py: 3,
              color: "text.secondary",
            }}
          >
            <Typography
              variant="caption"
              sx={{ opacity: 0.5, fontSize: "0.7rem" }}
            >
              {t("diagnostics.sipMessagesEmpty")}
            </Typography>
          </Box>
        )}
        {logs.map((log, i) => (
          <SipLogEntry
            key={`${log.timestamp}-${i}`}
            log={log}
            isSelected={selectedIdx === i}
            onToggle={() =>
              setSelectedIdx(selectedIdx === i ? null : i)
            }
          />
        ))}
      </Box>
    </>
  );
}

// --- Main Panel ---

export function DiagnosticPanel({ isDetached }: { isDetached?: boolean }) {
  const { t } = useTranslation();
  const theme = useTheme();
  const setCurrentView = useAppStore((s) => s.setCurrentView);
  const [logs, setLogs] = useState<DiagnosticLog[]>([]);
  const [status, setStatus] = useState<SystemStatus | null>(null);
  const [selectedTab, setSelectedTab] = useState(0);
  const [selectedIdx, setSelectedIdx] = useReducer(
    (_: number | null, v: number | null) => v,
    null,
  );
  const [exportMenuAnchor, setExportMenuAnchor] = useState<HTMLElement | null>(null);

  const fetchStatus = useCallback(async () => {
    try {
      const result = await invoke<SystemStatus | null>("get_system_status");
      setStatus(result);
    } catch {
      // ignore
    }
  }, []);

  const syncLogs = useCallback(async () => {
    try {
      const existing = await invoke<DiagnosticLog[]>("get_sip_diagnostics");
      setLogs(existing);
    } catch {
      // ignore
    }
  }, []);

  useEffect(() => {
    const initialLogTimer = setTimeout(syncLogs, 0);
    const logInterval = setInterval(syncLogs, 2000);

    const unlisten = listen<DiagnosticLog>("sip-diagnostic", () => {
      syncLogs();
    });

    const statusInterval = setInterval(fetchStatus, 2000);
    const initialStatusTimer = setTimeout(fetchStatus, 100);

    return () => {
      unlisten.then((fn_) => fn_());
      clearTimeout(initialLogTimer);
      clearInterval(logInterval);
      clearInterval(statusInterval);
      clearTimeout(initialStatusTimer);
    };
  }, [fetchStatus, syncLogs]);

  const handleClear = async () => {
    try {
      await invoke("clear_sip_diagnostics");
    } catch {
      // ignore
    }
    setLogs([]);
  };

  const handleExportText = async () => {
    try {
      const timestamp = Date.now();
      const filePath = await save({
        defaultPath: `aria-sip-${timestamp}.txt`,
        filters: [{ name: t("diagnostics.textFiles"), extensions: ["txt"] }],
        title: t("diagnostics.exportTextTitle"),
      });
      
      if (filePath) {
        await invoke("export_sip_log_text", { path: filePath });
      }
    } catch (e) {
      log.error("Failed to export text:", e);
    }
  };

  const handleExportPcap = async () => {
    try {
      const timestamp = Date.now();
      const filePath = await save({
        defaultPath: `aria-sip-${timestamp}.pcap`,
        filters: [{ name: t("diagnostics.pcapFiles"), extensions: ["pcap"] }],
        title: t("diagnostics.exportPcapTitle"),
      });
      
      if (filePath) {
        await invoke("export_sip_log_pcap", { path: filePath });
      }
    } catch (e) {
      log.error("Failed to export pcap:", e);
    }
  };

  const handleExportReport = useCallback(async () => {
    try {
      const callHistoryRaw = localStorage.getItem("aria_call_history");
      const callHistory = callHistoryRaw ? JSON.parse(callHistoryRaw) : [];
      const timestamp = Date.now();

      const filePath = await save({
        defaultPath: `aria-diagnostic-report-${timestamp}.json`,
        filters: [{ name: t("diagnostics.jsonFiles"), extensions: ["json"] }],
        title: t("diagnostics.exportReportTitle"),
      });

      if (filePath) {
        await invoke("export_diagnostic_report", { callHistory, path: filePath });
      }
    } catch (e) {
      log.error("Failed to export report:", e);
    }
    setExportMenuAnchor(null);
  }, [t]);

  const handleExportCallHistory = useCallback(async () => {
    try {
      const callHistoryRaw = localStorage.getItem("aria_call_history");
      const callHistory = callHistoryRaw ? JSON.parse(callHistoryRaw) : [];
      const timestamp = Date.now();

      const filePath = await save({
        defaultPath: `aria-call-history-${timestamp}.csv`,
        filters: [{ name: t("diagnostics.csvFiles"), extensions: ["csv"] }],
        title: t("diagnostics.exportHistoryTitle"),
      });

      if (filePath) {
        await invoke("export_call_history_csv", { callHistory, path: filePath });
      }
    } catch (e) {
      log.error("Failed to export call history:", e);
    }
    setExportMenuAnchor(null);
  }, [t]);

  const accounts = status?.accounts ?? [];
  const hasMultipleAccounts = accounts.length > 1;
  const selectedAccount = accounts[selectedTab] ?? accounts[0];
  const anyRegistering = accounts.some(a => a.registrationState === "registering");

  // Filter logs by accountId for the selected account
  const accountLogs = selectedAccount?.accountId
    ? logs.filter(l => l.accountId === selectedAccount.accountId)
    : logs;

  return (
    <Box
      sx={{
        height: "100%",
        display: "flex",
        flexDirection: "column",
        bgcolor: "background.default",
      }}
    >
      {/* Header */}
      <Box
        sx={{
          px: 1.5,
          pt: isDetached ? 1 : 1.5,
          pb: 0.5,
          display: "flex",
          alignItems: "center",
          gap: 1,
          ...(isDetached
            ? {
                WebkitAppRegion: "drag",
                userSelect: "none",
              }
            : {}),
        }}
      >
        {!isDetached && (
          <IconButton size="small" onClick={() => setCurrentView("settings")}>
            <ArrowBackIcon fontSize="small" />
          </IconButton>
        )}
        <Typography
          variant="subtitle2"
          sx={{
            fontWeight: 600,
            flex: 1,
            letterSpacing: "-0.01em",
            fontSize: "0.85rem",
          }}
        >
          {t("diagnostics.title")}
        </Typography>
        <Typography
          variant="caption"
          sx={{
            fontFamily: "monospace",
            fontSize: "0.55rem",
            color: "text.secondary",
            opacity: 0.5,
          }}
        >
          {t("diagnostics.version")}
        </Typography>
        <Tooltip title={t("diagnostics.export")}>
          <IconButton
            size="small"
            onClick={(e) => setExportMenuAnchor(e.currentTarget)}
            sx={{ WebkitAppRegion: "no-drag" }}
          >
            <FileDownloadOutlinedIcon sx={{ fontSize: 16 }} />
          </IconButton>
        </Tooltip>
        <Menu
          anchorEl={exportMenuAnchor}
          open={Boolean(exportMenuAnchor)}
          onClose={() => setExportMenuAnchor(null)}
          anchorOrigin={{ vertical: "bottom", horizontal: "right" }}
          transformOrigin={{ vertical: "top", horizontal: "right" }}
          sx={{
            "& .MuiPaper-root": {
              minWidth: 200,
              bgcolor: "background.paper",
            },
          }}
        >
          <MenuItem onClick={() => { handleExportText(); setExportMenuAnchor(null); }}>
            <ListItemIcon><DescriptionOutlinedIcon fontSize="small" /></ListItemIcon>
            <ListItemText primary={t("diagnostics.exportText")} secondary={t("diagnostics.sipLog")} />
          </MenuItem>
          <MenuItem onClick={() => { handleExportPcap(); setExportMenuAnchor(null); }}>
            <ListItemIcon><SwapVertIcon fontSize="small" /></ListItemIcon>
            <ListItemText primary={t("diagnostics.exportPcap")} secondary={t("diagnostics.wiresharkFormat")} />
          </MenuItem>
          <Divider />
          <MenuItem onClick={handleExportCallHistory}>
            <ListItemIcon><HistoryIcon fontSize="small" /></ListItemIcon>
            <ListItemText primary={t("diagnostics.exportHistory")} secondary={t("diagnostics.csvFormat")} />
          </MenuItem>
          <MenuItem onClick={handleExportReport}>
            <ListItemIcon><AssessmentOutlinedIcon fontSize="small" /></ListItemIcon>
            <ListItemText primary={t("diagnostics.exportReport")} secondary={t("diagnostics.fullDiagnostics")} />
          </MenuItem>
        </Menu>
        <Tooltip title={t("diagnostics.clearLog")}>
          <IconButton
            size="small"
            onClick={handleClear}
            sx={{ WebkitAppRegion: "no-drag" }}
          >
            <DeleteOutlineIcon sx={{ fontSize: 16 }} />
          </IconButton>
        </Tooltip>
      </Box>

      {anyRegistering && (
        <LinearProgress
          sx={{
            height: 2,
            mx: 1.5,
            borderRadius: 1,
            mb: 0.5,
          }}
        />
      )}

      {/* Account Tabs (only show if multiple accounts) */}
      {hasMultipleAccounts && (
        <Tabs
          value={selectedTab}
          onChange={(_, v) => {
            setSelectedTab(v);
            setSelectedIdx(null);
          }}
          variant="scrollable"
          scrollButtons="auto"
          sx={{
            minHeight: 32,
            mx: 1.5,
            mb: 0.5,
            "& .MuiTabs-indicator": {
              height: 2,
              borderRadius: 1,
            },
            "& .MuiTab-root": {
              minHeight: 32,
              minWidth: 0,
              py: 0.5,
              px: 1.5,
              fontSize: "0.7rem",
              fontWeight: 500,
              textTransform: "none",
            },
          }}
        >
          {accounts.map((account, idx) => (
            <Tab
              key={account.accountId}
              label={
                <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
                  <StatusDot color={registrationColor(account.registrationState)} />
                  <span>{account.username}@{account.domain}</span>
                </Box>
              }
              value={idx}
            />
          ))}
        </Tabs>
      )}

      {/* Scrollable content */}
      <Box
        sx={{
          flex: 1,
          overflow: "auto",
          pb: 1,
          "&::-webkit-scrollbar": { width: 4 },
          "&::-webkit-scrollbar-thumb": {
            bgcolor: alpha(theme.palette.text.primary, 0.08),
            borderRadius: 2,
          },
        }}
      >
        {selectedAccount ? (
          <AccountTabContent
            account={selectedAccount}
            logs={hasMultipleAccounts ? accountLogs : logs}
            latencyMs={status?.latencyMs ?? null}
            selectedIdx={selectedIdx}
            setSelectedIdx={setSelectedIdx}
          />
        ) : (
          <Box sx={{ textAlign: "center", py: 4 }}>
            <Typography variant="body2" sx={{ color: "text.secondary" }}>
              {t("diagnostics.noAccounts")}
            </Typography>
          </Box>
        )}
      </Box>
    </Box>
  );
}
