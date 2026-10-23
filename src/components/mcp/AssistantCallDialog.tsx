import { useEffect, useRef, useState } from "react";
import {
  Box,
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Typography,
} from "@mui/material";
import SmartToyOutlinedIcon from "@mui/icons-material/SmartToyOutlined";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { placeCall } from "../../hooks/useSip";
import { dialUri } from "../../utils/sipUri";
import { log } from "../../utils/log";

/** What the backend sends when an assistant asks to place a call. */
interface CallRequest {
  requestId: string;
  number: string;
  accountId: string | null;
}

/** Same as the backend's consent timeout; the dialog closes itself then. */
const TIMEOUT_SECONDS = 30;

async function respond(
  requestId: string,
  decision: { allow: boolean; callId?: string; error?: string },
): Promise<void> {
  try {
    await invoke("mcp_call_decision", {
      requestId,
      allow: decision.allow,
      callId: decision.callId ?? null,
      error: decision.error ?? null,
    });
  } catch (e) {
    // A request that already timed out is the usual cause: nothing to do.
    log.warn("[MCP] Could not report the call decision:", e);
  }
}

/**
 * The consent dialog for calls requested over MCP.
 *
 * An assistant never dials on its own: the backend emits `mcp-call-request`,
 * this asks the user, and only on Allow is the call placed — through the
 * same `placeCall` + `dialUri` path as the dialer, so the account's dialing
 * rules apply. Deny, or no answer within the timeout, reports a denial.
 */
export function AssistantCallDialog() {
  const { t } = useTranslation();
  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const defaultCountry = useAppStore((s) => s.defaultCountry);

  const [request, setRequest] = useState<CallRequest | null>(null);
  const [secondsLeft, setSecondsLeft] = useState(TIMEOUT_SECONDS);
  const [busy, setBusy] = useState(false);
  // The request on screen, readable from the event handler without a
  // re-subscribe: a second request while one is open is denied at once
  // rather than replacing the dialog under the user's cursor.
  const pendingRef = useRef<string | null>(null);
  // Closes the dialog when the backend's timeout has passed.
  const deadlineRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const unlisten = listen<CallRequest>("mcp-call-request", (event) => {
      const incoming = event.payload;
      if (pendingRef.current) {
        void respond(incoming.requestId, {
          allow: false,
          error: "another call request is waiting",
        });
        return;
      }
      pendingRef.current = incoming.requestId;
      setRequest(incoming);
      setSecondsLeft(TIMEOUT_SECONDS);
      // Out of time: the backend has denied it by then; close to match.
      deadlineRef.current = setTimeout(() => {
        if (pendingRef.current !== incoming.requestId) return;
        pendingRef.current = null;
        setRequest(null);
        setBusy(false);
        void respond(incoming.requestId, { allow: false });
      }, TIMEOUT_SECONDS * 1000);
    });
    return () => {
      unlisten.then((fn) => fn());
      if (deadlineRef.current) clearTimeout(deadlineRef.current);
    };
  }, []);

  // Count down while a request is open.
  useEffect(() => {
    if (!request) return;
    const timer = setInterval(() => setSecondsLeft((s) => Math.max(0, s - 1)), 1000);
    return () => clearInterval(timer);
  }, [request]);

  const account =
    accounts.find((a) => a.id === request?.accountId) ??
    accounts.find((a) => a.id === activeAccountId) ??
    accounts[0];

  const close = () => {
    if (deadlineRef.current) {
      clearTimeout(deadlineRef.current);
      deadlineRef.current = null;
    }
    pendingRef.current = null;
    setRequest(null);
    setBusy(false);
  };

  const handleDeny = () => {
    if (!request) return;
    void respond(request.requestId, { allow: false });
    close();
  };

  const handleAllow = async () => {
    if (!request) return;
    if (!account) {
      await respond(request.requestId, { allow: true, error: t("mcp.noAccount") });
      close();
      return;
    }
    setBusy(true);
    try {
      const callId = await placeCall({
        uri: dialUri(request.number, account, defaultCountry),
        accountId: account.id,
      });
      await respond(request.requestId, { allow: true, callId });
    } catch (e) {
      await respond(request.requestId, { allow: true, error: String(e) });
    }
    close();
  };

  const accountLabel = account ? account.displayName || account.username : "—";

  return (
    <Dialog
      open={request !== null}
      onClose={busy ? undefined : handleDeny}
      maxWidth="xs"
      fullWidth
      slotProps={{
        paper: { sx: { borderRadius: "20px" } },
      }}
    >
      <DialogTitle sx={{ display: "flex", alignItems: "center", gap: 1.5 }}>
        <SmartToyOutlinedIcon sx={{ fontSize: 22, color: "primary.main" }} />
        {t("mcp.callRequestTitle")}
      </DialogTitle>
      <DialogContent>
        <Typography variant="body2" sx={{ mb: 1 }}>
          {t("mcp.callRequestBody", { number: request?.number ?? "", account: accountLabel })}
        </Typography>
        <Box
          sx={{
            px: 1.5,
            py: 1,
            borderRadius: "12px",
            bgcolor: "action.hover",
            fontFamily: "monospace",
            fontSize: "1.1rem",
            wordBreak: "break-all",
          }}
        >
          {request?.number}
        </Box>
        <Typography variant="caption" sx={{ color: "text.secondary", display: "block", mt: 1 }}>
          {t("mcp.callRequestExpires", { seconds: Math.max(0, secondsLeft) })}
        </Typography>
      </DialogContent>
      <DialogActions sx={{ px: 3, pb: 2 }}>
        <Button onClick={handleDeny} disabled={busy} sx={{ borderRadius: "10px" }}>
          {t("mcp.deny")}
        </Button>
        <Button
          onClick={() => void handleAllow()}
          variant="contained"
          disabled={busy}
          sx={{ borderRadius: "10px" }}
        >
          {t("mcp.allow")}
        </Button>
      </DialogActions>
    </Dialog>
  );
}
