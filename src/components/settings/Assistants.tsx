import { useEffect, useState } from "react";
import {
  Box,
  Button,
  IconButton,
  Switch,
  TextField,
  Tooltip,
  Typography,
} from "@mui/material";
import ContentCopyIcon from "@mui/icons-material/ContentCopy";
import AutorenewIcon from "@mui/icons-material/Autorenew";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { log } from "../../utils/log";

/** What the backend reports, mirroring `McpStatus` in `mcp/mod.rs`. */
interface McpStatus {
  enabled: boolean;
  port: number;
  allowCallControl: boolean;
  running: boolean;
}

const DEFAULT_PORT = 48060;

/** A port the backend will accept; the default if the field is empty or junk. */
function clampPort(value: string): number {
  const n = parseInt(value, 10);
  if (!Number.isFinite(n) || n < 1) return DEFAULT_PORT;
  return Math.min(n, 65535);
}

async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch (e) {
    log.warn("[MCP] Clipboard write failed:", e);
    return false;
  }
}

/**
 * Settings for the local MCP server: whether assistants may read status
 * and traces, whether they may control calls, the port, the token, and the
 * one-line setup for Claude Code.
 *
 * The token is shown masked. Copy puts it on the clipboard; Regenerate
 * replaces it at once, so an assistant set up with the old one stops
 * working until it is given the new one.
 */
export function Assistants() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [token, setToken] = useState<string | null>(null);
  const [portInput, setPortInput] = useState(String(DEFAULT_PORT));
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<"token" | "setup" | null>(null);
  const [working, setWorking] = useState(false);

  useEffect(() => {
    invoke<McpStatus>("mcp_get_config")
      .then((s) => {
        setStatus(s);
        setPortInput(String(s.port));
        if (s.enabled) {
          invoke<string | null>("mcp_get_token").then(setToken).catch(() => {});
        }
      })
      .catch((e) => log.error("Failed to load MCP settings:", e));
  }, []);

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(null), 1500);
    return () => clearTimeout(timer);
  }, [copied]);

  const apply = async (next: { enabled: boolean; port: number; allowCallControl: boolean }) => {
    setWorking(true);
    setError(null);
    try {
      const applied = await invoke<McpStatus>("mcp_set_config", { config: next });
      setStatus(applied);
      setPortInput(String(applied.port));
      if (applied.enabled) {
        setToken(await invoke<string | null>("mcp_get_token"));
      }
    } catch (e) {
      setError(String(e));
      // The backend switched itself back off; show that.
      invoke<McpStatus>("mcp_get_config").then(setStatus).catch(() => {});
    } finally {
      setWorking(false);
    }
  };

  const handleRegenerate = async () => {
    setWorking(true);
    try {
      setToken(await invoke<string>("mcp_rotate_token"));
    } catch (e) {
      setError(String(e));
    } finally {
      setWorking(false);
    }
  };

  const handleCopy = async (what: "token" | "setup", text: string) => {
    if (await copyText(text)) setCopied(what);
  };

  if (!status) {
    return (
      <Typography variant="caption" sx={{ color: "text.secondary" }}>
        {t("common.loading")}
      </Typography>
    );
  }

  const port = status.port;
  const setupLine = `claude mcp add --transport http aria http://127.0.0.1:${port}/mcp --header "Authorization: Bearer ${token ?? "<token>"}"`;

  const rowSx = {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: 2,
    py: 0.75,
  };

  return (
    <Box>
      <Typography variant="caption" sx={{ color: "text.secondary", display: "block", mb: 1 }}>
        {t("mcp.intro")}
      </Typography>

      {/* Read access */}
      <Box sx={rowSx}>
        <Typography variant="body2" sx={{ fontSize: "0.85rem" }}>
          {t("mcp.enableRead")}
        </Typography>
        <Switch
          size="small"
          checked={status.enabled}
          disabled={working}
          onChange={(e) =>
            void apply({
              enabled: e.target.checked,
              port: clampPort(portInput),
              allowCallControl: status.allowCallControl,
            })
          }
        />
      </Box>

      {/* Call control */}
      <Box sx={rowSx}>
        <Box>
          <Typography variant="body2" sx={{ fontSize: "0.85rem" }}>
            {t("mcp.enableControl")}
          </Typography>
          <Typography variant="caption" sx={{ color: "text.secondary" }}>
            {t("mcp.enableControlNote")}
          </Typography>
        </Box>
        <Switch
          size="small"
          checked={status.enabled && status.allowCallControl}
          disabled={!status.enabled || working}
          onChange={(e) =>
            void apply({
              enabled: status.enabled,
              port: clampPort(portInput),
              allowCallControl: e.target.checked,
            })
          }
        />
      </Box>

      {/* Port */}
      <Box sx={rowSx}>
        <Box>
          <Typography variant="body2" sx={{ fontSize: "0.85rem" }}>
            {t("mcp.port")}
          </Typography>
          <Typography variant="caption" sx={{ color: "text.secondary" }}>
            {t("mcp.portNote")}
          </Typography>
        </Box>
        <TextField
          size="small"
          value={portInput}
          disabled={working}
          onChange={(e) => setPortInput(e.target.value.replace(/\D/g, ""))}
          onBlur={() => {
            const next = clampPort(portInput);
            setPortInput(String(next));
            if (next !== status.port) {
              void apply({
                enabled: status.enabled,
                port: next,
                allowCallControl: status.allowCallControl,
              });
            }
          }}
          slotProps={{ htmlInput: { inputMode: "numeric", style: { width: 72, textAlign: "right" } } }}
          sx={{ "& .MuiOutlinedInput-root": { borderRadius: "10px" } }}
        />
      </Box>

      {error && (
        <Typography variant="caption" sx={{ color: "error.main", display: "block", my: 0.5 }}>
          {t("mcp.notRunning", { error })}
        </Typography>
      )}

      {status.enabled && (
        <>
          {/* Token */}
          <Box sx={rowSx}>
            <Box sx={{ minWidth: 0 }}>
              <Typography variant="body2" sx={{ fontSize: "0.85rem" }}>
                {t("mcp.token")}
              </Typography>
              <Typography
                variant="caption"
                sx={{ color: "text.secondary", fontFamily: "monospace", letterSpacing: 1 }}
              >
                {token ? "••••••••••••••••••••" : t("common.loading")}
              </Typography>
            </Box>
            <Box sx={{ display: "flex", gap: 0.5 }}>
              <Tooltip title={copied === "token" ? t("mcp.copied") : t("mcp.copy")}>
                <span>
                  <IconButton
                    size="small"
                    disabled={!token || working}
                    onClick={() => token && void handleCopy("token", token)}
                    aria-label={t("mcp.copy")}
                  >
                    <ContentCopyIcon sx={{ fontSize: 18 }} />
                  </IconButton>
                </span>
              </Tooltip>
              <Tooltip title={t("mcp.regenerateNote")}>
                <span>
                  <Button
                    size="small"
                    startIcon={<AutorenewIcon sx={{ fontSize: 16 }} />}
                    disabled={working}
                    onClick={() => void handleRegenerate()}
                    sx={{ borderRadius: "10px", textTransform: "none" }}
                  >
                    {t("mcp.regenerate")}
                  </Button>
                </span>
              </Tooltip>
            </Box>
          </Box>

          {/* Setup line */}
          <Box sx={{ mt: 1 }}>
            <Box sx={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
              <Typography variant="body2" sx={{ fontSize: "0.85rem" }}>
                {t("mcp.setupLine")}
              </Typography>
              <Tooltip title={copied === "setup" ? t("mcp.copied") : t("mcp.copy")}>
                <span>
                  <IconButton
                    size="small"
                    disabled={!token}
                    onClick={() => void handleCopy("setup", setupLine)}
                    aria-label={t("mcp.copy")}
                  >
                    <ContentCopyIcon sx={{ fontSize: 18 }} />
                  </IconButton>
                </span>
              </Tooltip>
            </Box>
            <Box
              component="pre"
              sx={{
                m: 0,
                mt: 0.5,
                p: 1.25,
                borderRadius: "12px",
                bgcolor: "action.hover",
                fontFamily: "monospace",
                fontSize: "0.72rem",
                whiteSpace: "pre-wrap",
                wordBreak: "break-all",
                userSelect: "all",
              }}
            >
              {token ? setupLine : setupLine.replace(/Bearer .*"/, 'Bearer …"')}
            </Box>
            <Typography variant="caption" sx={{ color: "text.secondary", display: "block", mt: 0.5 }}>
              {t("mcp.setupNote")}
            </Typography>
          </Box>
        </>
      )}
    </Box>
  );
}
