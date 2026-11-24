import { useEffect, useMemo, useRef, useState } from "react";
import {
  Box,
  Chip,
  Collapse,
  Divider,
  IconButton,
  InputAdornment,
  LinearProgress,
  ListItemIcon,
  ListItemText,
  Menu,
  MenuItem,
  Tab,
  Tabs,
  TextField,
  ToggleButton,
  ToggleButtonGroup,
  Tooltip,
  Typography,
  alpha,
  useTheme,
} from "@mui/material";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutlineOutlined";
import FileDownloadOutlinedIcon from "@mui/icons-material/FileDownloadOutlined";
import DescriptionOutlinedIcon from "@mui/icons-material/DescriptionOutlined";
import SwapVertIcon from "@mui/icons-material/SwapVert";
import HistoryIcon from "@mui/icons-material/History";
import AssessmentOutlinedIcon from "@mui/icons-material/AssessmentOutlined";
import ViewListIcon from "@mui/icons-material/ViewList";
import AccountTreeIcon from "@mui/icons-material/AccountTree";
import PushPinIcon from "@mui/icons-material/PushPin";
import PushPinOutlinedIcon from "@mui/icons-material/PushPinOutlined";
import VerticalAlignBottomIcon from "@mui/icons-material/VerticalAlignBottom";
import SearchIcon from "@mui/icons-material/Search";
import CloseIcon from "@mui/icons-material/Close";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import ExpandLessIcon from "@mui/icons-material/ExpandLess";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { useAppStore, getAccountWithPassword } from "../../stores/appStore";
import { useDiagnosticsStore } from "../../stores/diagnosticsStore";
import { errorText, notifyError } from "../../stores/toastStore";
import { sipRegister, sipReregister, sipUnregisterAccount } from "../../hooks/useSip";
import { filterLogs } from "../../utils/diagnosticsFilter";
import { callIdTag, type MessageCategory } from "../../utils/sipMessage";
import { log as applog } from "../../utils/log";
import { AppToast } from "../layout/AppToast";
import { ActiveCallCard, ConnectionCard, type AccountActions } from "./StatusCards";
import { registrationColor } from "./registrationColor";
import { MessageRow } from "./MessageRow";
import { SipLadderDiagram } from "./SipLadder";
import { logKey } from "./types";

const CATEGORIES: { key: MessageCategory; label: string }[] = [
  { key: "calls", label: "Calls" },
  { key: "register", label: "Register" },
  { key: "keepalive", label: "OPTIONS" },
  { key: "other", label: "Other" },
];

/**
 * The Diagnostics window: the account's connection and calls at the top,
 * the SIP trace below, with millisecond timing, filters, search and
 * per-dialog views. Docked beside the main window by default; the pin
 * undocks it. It runs in its own webview, so everything it shows comes from
 * the backend (polled status, pushed trace), not the main window's store.
 */
export function DiagnosticPanel() {
  const { t } = useTranslation();
  const theme = useTheme();
  const store = useDiagnosticsStore();
  const start = useDiagnosticsStore((s) => s.start);
  const accounts = useAppStore((s) => s.accounts);
  const [exportMenuAnchor, setExportMenuAnchor] = useState<HTMLElement | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => start(), [start]);

  const statusAccounts = store.status?.accounts ?? [];
  const selected =
    statusAccounts.find((a) => a.accountId === store.selectedAccount) ?? statusAccounts[0] ?? null;
  const hasMultiple = statusAccounts.length > 1;
  const anyRegistering = statusAccounts.some((a) => a.registrationState === "registering");

  const shown = useMemo(
    () =>
      filterLogs(store.logs, {
        accountId: hasMultiple ? selected?.accountId : undefined,
        categories: store.categories,
        search: store.search,
        callId: store.callId,
      }),
    [store.logs, hasMultiple, selected?.accountId, store.categories, store.search, store.callId],
  );

  // Follow: keep the newest message in view as they arrive.
  useEffect(() => {
    if (!store.follow) return;
    const el = listRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [shown.length, store.follow, store.view]);

  const run = async (accountId: string, action: () => Promise<unknown>) => {
    setBusy(accountId);
    try {
      await action();
    } catch (e) {
      notifyError(errorText(e));
    } finally {
      setBusy(null);
    }
  };
  const withPassword = async (accountId: string) => {
    const account = accounts.find((a) => a.id === accountId);
    if (!account) throw new Error(t("diagnostics.noAccounts"));
    return getAccountWithPassword(account);
  };
  const actions: AccountActions = {
    busy,
    reregister: (id) => void run(id, async () => sipReregister(await withPassword(id))),
    unregister: (id) => void run(id, () => sipUnregisterAccount(id)),
    register: (id) => void run(id, async () => sipRegister(await withPassword(id))),
  };

  // The backend opens the save dialog itself, so the destination path never
  // passes through the webview. Only the translated labels are sent.
  const exportWith = (command: string, args: Record<string, unknown>) => {
    setExportMenuAnchor(null);
    invoke(command, args).catch((e) => applog.error(`[diagnostics] ${command} failed:`, e));
  };
  const callHistory = () => {
    try {
      return JSON.parse(localStorage.getItem("aria_call_history") ?? "[]");
    } catch {
      return [];
    }
  };

  const info = store.appInfo;
  const pickedTag = store.callId ? callIdTag(store.callId) : null;

  return (
    <Box sx={{ height: "100vh", display: "flex", flexDirection: "column", bgcolor: "background.default" }}>
      {/* Title row */}
      <Box sx={{ px: 1.5, pt: 1, pb: 0.5, display: "flex", alignItems: "center", gap: 1, WebkitAppRegion: "drag", userSelect: "none" }}>
        <Typography variant="subtitle2" sx={{ fontWeight: 600 }}>
          {t("diagnostics.title")}
        </Typography>
        {info && (
          <Tooltip title={`${info.os} ${info.arch} · ${info.profile}${info.transcription ? ` · ${t("diagnostics.transcriptionOn")}` : ""}`}>
            <Typography variant="caption" sx={{ fontFamily: "monospace", fontSize: "0.62rem", color: "text.secondary" }}>
              {t("diagnostics.version", { version: info.version })}
            </Typography>
          </Tooltip>
        )}
        <Box sx={{ flex: 1 }} />
        <Tooltip title={store.docked ? t("diagnostics.undock") : t("diagnostics.dock")}>
          <IconButton size="small" onClick={() => void store.setDocked(!store.docked)} sx={{ WebkitAppRegion: "no-drag" }} aria-pressed={store.docked}>
            {store.docked ? <PushPinIcon sx={{ fontSize: 16 }} /> : <PushPinOutlinedIcon sx={{ fontSize: 16 }} />}
          </IconButton>
        </Tooltip>
        <Tooltip title={t("diagnostics.follow")}>
          <IconButton
            size="small"
            onClick={() => store.setFollow(!store.follow)}
            sx={{ WebkitAppRegion: "no-drag", color: store.follow ? "primary.main" : "text.secondary" }}
            aria-pressed={store.follow}
          >
            <VerticalAlignBottomIcon sx={{ fontSize: 16 }} />
          </IconButton>
        </Tooltip>
        <Tooltip title={t("diagnostics.export")}>
          <IconButton size="small" onClick={(e) => setExportMenuAnchor(e.currentTarget)} sx={{ WebkitAppRegion: "no-drag" }}>
            <FileDownloadOutlinedIcon sx={{ fontSize: 16 }} />
          </IconButton>
        </Tooltip>
        <Menu
          anchorEl={exportMenuAnchor}
          open={Boolean(exportMenuAnchor)}
          onClose={() => setExportMenuAnchor(null)}
          anchorOrigin={{ vertical: "bottom", horizontal: "right" }}
          transformOrigin={{ vertical: "top", horizontal: "right" }}
          sx={{ "& .MuiPaper-root": { minWidth: 220, bgcolor: "background.paper" } }}
        >
          <MenuItem onClick={() => exportWith("export_sip_log_text", { title: t("diagnostics.exportTextTitle"), filterName: t("diagnostics.textFiles") })}>
            <ListItemIcon><DescriptionOutlinedIcon fontSize="small" /></ListItemIcon>
            <ListItemText primary={t("diagnostics.exportText")} secondary={t("diagnostics.sipLog")} />
          </MenuItem>
          <MenuItem onClick={() => exportWith("export_sip_log_pcap", { title: t("diagnostics.exportPcapTitle"), filterName: t("diagnostics.pcapFiles") })}>
            <ListItemIcon><SwapVertIcon fontSize="small" /></ListItemIcon>
            <ListItemText primary={t("diagnostics.exportPcap")} secondary={t("diagnostics.wiresharkFormat")} />
          </MenuItem>
          <Divider />
          <MenuItem onClick={() => exportWith("export_call_history_csv", { callHistory: callHistory(), title: t("diagnostics.exportHistoryTitle"), filterName: t("diagnostics.csvFiles") })}>
            <ListItemIcon><HistoryIcon fontSize="small" /></ListItemIcon>
            <ListItemText primary={t("diagnostics.exportHistory")} secondary={t("diagnostics.csvFormat")} />
          </MenuItem>
          <MenuItem onClick={() => exportWith("export_diagnostic_report", { callHistory: callHistory(), title: t("diagnostics.exportReportTitle"), filterName: t("diagnostics.jsonFiles") })}>
            <ListItemIcon><AssessmentOutlinedIcon fontSize="small" /></ListItemIcon>
            <ListItemText primary={t("diagnostics.exportReport")} secondary={t("diagnostics.fullDiagnostics")} />
          </MenuItem>
        </Menu>
        <Tooltip title={t("diagnostics.clearLog")}>
          <IconButton size="small" onClick={() => void store.clear()} sx={{ WebkitAppRegion: "no-drag" }}>
            <DeleteOutlineIcon sx={{ fontSize: 16 }} />
          </IconButton>
        </Tooltip>
      </Box>

      {/* Account tabs */}
      {hasMultiple && (
        <Tabs
          value={selected?.accountId ?? false}
          onChange={(_, v) => store.selectAccount(v)}
          variant="scrollable"
          scrollButtons="auto"
          sx={{
            minHeight: 30,
            mx: 1.5,
            "& .MuiTabs-indicator": { height: 2, borderRadius: 1 },
            "& .MuiTab-root": { minHeight: 30, minWidth: 0, py: 0.25, px: 1.25, fontSize: "0.7rem", fontWeight: 500, textTransform: "none" },
          }}
        >
          {statusAccounts.map((account) => (
            <Tab
              key={account.accountId}
              value={account.accountId}
              label={
                <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
                  <Box sx={{ width: 6, height: 6, borderRadius: "50%", bgcolor: registrationColor(account.registrationState) }} />
                  <span>{account.username}@{account.domain}</span>
                </Box>
              }
            />
          ))}
        </Tabs>
      )}
      {anyRegistering && <LinearProgress sx={{ height: 2, mx: 1.5, borderRadius: 1 }} />}

      {/* Status cards, collapsible */}
      <Box sx={{ px: 1.5, pt: 0.5 }}>
        <Box
          onClick={store.toggleStatus}
          sx={{ display: "flex", alignItems: "center", gap: 0.5, cursor: "pointer", color: "text.secondary", userSelect: "none" }}
          role="button"
          tabIndex={0}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              store.toggleStatus();
            }
          }}
          aria-expanded={store.showStatus}
        >
          {store.showStatus ? <ExpandLessIcon sx={{ fontSize: 14 }} /> : <ExpandMoreIcon sx={{ fontSize: 14 }} />}
          <Typography variant="caption" sx={{ fontWeight: 600, fontSize: "0.66rem", textTransform: "uppercase", letterSpacing: "0.05em" }}>
            {t("diagnostics.status")}
          </Typography>
          {!store.showStatus && selected && (
            <Typography variant="caption" sx={{ fontSize: "0.66rem", ml: 0.5, color: registrationColor(selected.registrationState), fontWeight: 600 }}>
              {selected.username}@{selected.domain} · {t(`diagnostics.${selected.registrationState}`, { defaultValue: selected.registrationState })}
            </Typography>
          )}
        </Box>
        <Collapse in={store.showStatus}>
          {selected ? (
            <>
              <ConnectionCard account={selected} latencyMs={store.status?.latencyMs ?? null} actions={actions} />
              {selected.activeCalls.map((call) => (
                <ActiveCallCard key={call.id} call={call} />
              ))}
            </>
          ) : (
            <Typography variant="body2" sx={{ color: "text.secondary", py: 1 }}>
              {t("diagnostics.noAccounts")}
            </Typography>
          )}
        </Collapse>
      </Box>

      {/* Trace toolbar */}
      <Box sx={{ display: "flex", alignItems: "center", gap: 0.75, px: 1.5, pt: 0.75, pb: 0.5, flexWrap: "wrap" }}>
        <TextField
          size="small"
          value={store.search}
          onChange={(e) => store.setSearch(e.target.value)}
          placeholder={t("diagnostics.search")}
          slotProps={{
            input: {
              startAdornment: (
                <InputAdornment position="start">
                  <SearchIcon sx={{ fontSize: 14 }} />
                </InputAdornment>
              ),
              endAdornment: store.search ? (
                <InputAdornment position="end">
                  <IconButton size="small" onClick={() => store.setSearch("")} aria-label={t("common.clear")}>
                    <CloseIcon sx={{ fontSize: 12 }} />
                  </IconButton>
                </InputAdornment>
              ) : undefined,
              sx: { height: 26, fontSize: "0.72rem", borderRadius: "8px" },
            },
          }}
          sx={{ flex: 1, minWidth: 140 }}
        />
        <ToggleButtonGroup
          value={store.view}
          exclusive
          onChange={(_, v) => {
            if (v) store.setView(v);
          }}
          size="small"
          sx={{ height: 26, "& .MuiToggleButton-root": { px: 0.75, py: 0, border: `1px solid ${alpha(theme.palette.divider, 0.2)}` } }}
        >
          <ToggleButton value="messages" aria-label={t("diagnostics.messageView")}>
            <Tooltip title={t("diagnostics.messageView")}><ViewListIcon sx={{ fontSize: 14 }} /></Tooltip>
          </ToggleButton>
          <ToggleButton value="ladder" aria-label={t("diagnostics.ladderView")}>
            <Tooltip title={t("diagnostics.ladderView")}><AccountTreeIcon sx={{ fontSize: 14 }} /></Tooltip>
          </ToggleButton>
        </ToggleButtonGroup>
      </Box>
      <Box sx={{ display: "flex", gap: 0.5, px: 1.5, pb: 0.5, flexWrap: "wrap", alignItems: "center" }}>
        {CATEGORIES.map(({ key, label }) => {
          const active = store.categories.has(key);
          return (
            <Chip
              key={key}
              label={label}
              size="small"
              variant={active ? "filled" : "outlined"}
              onClick={() => store.toggleCategory(key)}
              sx={{
                height: 20,
                fontSize: "0.6rem",
                fontWeight: 600,
                borderRadius: "10px",
                "& .MuiChip-label": { px: 0.75 },
                opacity: active ? 1 : 0.45,
                bgcolor: active ? alpha(theme.palette.primary.main, 0.12) : "transparent",
                color: active ? "primary.main" : "text.secondary",
                borderColor: active ? alpha(theme.palette.primary.main, 0.3) : alpha(theme.palette.divider, 0.3),
              }}
            />
          );
        })}
        {pickedTag && (
          <Chip
            label={t("diagnostics.callOnly", { tag: pickedTag.text })}
            size="small"
            onDelete={() => store.setCallId(null)}
            sx={{ height: 20, fontSize: "0.6rem", fontFamily: "monospace", bgcolor: alpha(`hsl(${pickedTag.hue} 70% 50%)`, 0.18) }}
          />
        )}
        <Typography variant="caption" sx={{ ml: "auto", fontSize: "0.62rem", color: "text.secondary" }}>
          {t("diagnostics.sipMessages", { count: shown.length })}
        </Typography>
      </Box>

      {/* Trace */}
      <Box
        ref={listRef}
        sx={{
          flex: 1,
          overflow: "auto",
          px: 1,
          pb: 1,
          "&::-webkit-scrollbar": { width: 4 },
          "&::-webkit-scrollbar-thumb": { bgcolor: alpha(theme.palette.text.primary, 0.08), borderRadius: 2 },
        }}
      >
        {shown.length === 0 ? (
          <Box sx={{ textAlign: "center", py: 4, color: "text.secondary" }}>
            <Typography variant="caption" sx={{ opacity: 0.6, fontSize: "0.7rem" }}>
              {store.logs.length === 0 ? t("diagnostics.sipMessagesEmpty") : t("diagnostics.noMatches")}
            </Typography>
          </Box>
        ) : store.view === "ladder" ? (
          <SipLadderDiagram logs={shown.filter((l) => l.direction !== "app")} />
        ) : (
          shown.map((entry, i) => {
            const key = logKey(entry);
            return (
              <MessageRow
                key={`${key}-${i}`}
                log={entry}
                previousTimestamp={i > 0 ? shown[i - 1].timestamp : null}
                expanded={store.expanded.has(key)}
                onToggle={() => store.toggleExpanded(key)}
                onPickCall={(callId) => store.setCallId(store.callId === callId ? null : callId)}
                pickedCall={store.callId !== null && store.callId === entry.callId}
              />
            );
          })
        )}
      </Box>
      {!store.follow && shown.length > 0 && (
        <Box sx={{ position: "absolute", bottom: 12, right: 16 }}>
          <Chip
            label={t("diagnostics.jumpToNewest")}
            size="small"
            color="primary"
            onClick={() => store.setFollow(true)}
            icon={<VerticalAlignBottomIcon sx={{ fontSize: 14 }} />}
          />
        </Box>
      )}
      <AppToast />
    </Box>
  );
}
