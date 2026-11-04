import { useEffect, useState } from "react";
import {
  Box,
  Button,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Typography,
} from "@mui/material";
import { useStore } from "zustand";
import { useTranslation } from "react-i18next";
import { provisionFlow } from "./provisionApi";
import type { ProvisionPreview } from "../../stores/provisionFlow";

/**
 * How long Add stays disabled after the preview appears. The dialog can pop
 * up under a cursor that is already clicking (any page can fire the link), so
 * a click meant for something else must not land on Add.
 */
const ARM_DELAY_MS = 800;

function Row({ label, value }: { label: string; value: string }) {
  if (!value) return null;
  return (
    <Box sx={{ display: "flex", gap: 1.5, py: 0.5 }}>
      <Typography variant="body2" sx={{ color: "text.secondary", minWidth: 96 }}>
        {label}
      </Typography>
      <Typography variant="body2" sx={{ wordBreak: "break-word" }}>
        {value}
      </Typography>
    </Box>
  );
}

function PreviewDetails({ preview }: { preview: ProvisionPreview }) {
  const { t } = useTranslation();
  const line = preview.displayName ? `${preview.extension} · ${preview.displayName}` : preview.extension;
  return (
    <>
      <Box sx={{ px: 1.5, py: 1.25, mb: 1.5, borderRadius: "12px", bgcolor: "action.hover" }}>
        <Typography variant="caption" sx={{ color: "text.secondary", display: "block" }}>
          {t("provision.setUpBy")}
        </Typography>
        <Typography variant="h6" sx={{ fontSize: "1.1rem", fontWeight: 600, wordBreak: "break-all" }}>
          {preview.serverHost}
        </Typography>
      </Box>
      <Row label={t("provision.organization")} value={preview.tenantName} />
      <Row label={t("provision.line")} value={line} />
      <Row label={t("provision.domain")} value={preview.domain} />
      <Typography variant="caption" sx={{ color: "text.secondary", display: "block", mt: 1.5 }}>
        {t("provision.trustNote")}
      </Typography>
    </>
  );
}

/** Add stays disabled briefly after mounting; see ARM_DELAY_MS. */
function AddButton({ onClick }: { onClick: () => void }) {
  const { t } = useTranslation();
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    const timer = setTimeout(() => setArmed(true), ARM_DELAY_MS);
    return () => clearTimeout(timer);
  }, []);
  return (
    <Button onClick={onClick} variant="contained" disabled={!armed} sx={{ borderRadius: "10px" }}>
      {t("provision.addLine")}
    </Button>
  );
}

function Progress({ text }: { text: string }) {
  return (
    <Box sx={{ display: "flex", alignItems: "center", gap: 1.5, py: 1 }}>
      <CircularProgress size={20} />
      <Typography variant="body2">{text}</Typography>
    </Box>
  );
}

/**
 * The confirm dialog for one-click setup links (deep link or QR code).
 *
 * Shows who is setting the line up and what it is, and redeems the link only
 * when the user clicks Add line. Cancel closes it without using the link up.
 */
export function ProvisionLinkDialog() {
  const { t } = useTranslation();
  const state = useStore(provisionFlow, (s) => s.state);
  const { confirm, cancel } = provisionFlow.getState();

  const busy = state.phase === "redeeming" || state.phase === "registering";
  let body: React.ReactNode = null;
  let actions: React.ReactNode = null;

  switch (state.phase) {
    case "idle":
      break;
    case "loading":
      body = <Progress text={t("provision.checking")} />;
      actions = <Button onClick={cancel}>{t("common.cancel")}</Button>;
      break;
    case "preview":
      body = <PreviewDetails preview={state.preview} />;
      actions = (
        <>
          <Button onClick={cancel} sx={{ borderRadius: "10px" }}>
            {t("common.cancel")}
          </Button>
          <AddButton key={state.link.redeemUrl} onClick={() => void confirm()} />
        </>
      );
      break;
    case "redeeming":
    case "registering":
      body = (
        <>
          <PreviewDetails preview={state.preview} />
          <Progress text={state.phase === "redeeming" ? t("provision.adding") : t("provision.connecting")} />
        </>
      );
      actions = (
        <Button variant="contained" disabled sx={{ borderRadius: "10px" }}>
          {t("provision.addLine")}
        </Button>
      );
      break;
    case "done":
      body = state.error ? (
        <>
          <Typography variant="body2">{t("provision.notConnected")}</Typography>
          <Typography variant="body2" sx={{ color: "error.main", mt: 1, wordBreak: "break-word" }}>
            {state.error}
          </Typography>
        </>
      ) : (
        <Typography variant="body2">{t("provision.ready")}</Typography>
      );
      actions = (
        <Button onClick={cancel} variant="contained" sx={{ borderRadius: "10px" }}>
          {t("provision.done")}
        </Button>
      );
      break;
    case "failed":
      body = (
        <Typography variant="body2" sx={{ color: "error.main", wordBreak: "break-word" }}>
          {state.message}
        </Typography>
      );
      actions = <Button onClick={cancel}>{t("provision.close")}</Button>;
      break;
  }

  const title =
    state.phase === "done"
      ? t(state.line.updated ? "provision.updated" : "provision.added", { ext: state.line.username })
      : state.phase === "failed"
        ? t("provision.failedTitle")
        : t("provision.title");

  return (
    <Dialog
      open={state.phase !== "idle"}
      onClose={busy ? undefined : cancel}
      maxWidth="xs"
      fullWidth
      slotProps={{ paper: { sx: { borderRadius: "20px" } } }}
    >
      <DialogTitle>{title}</DialogTitle>
      <DialogContent>{body}</DialogContent>
      <DialogActions sx={{ px: 3, pb: 2 }}>{actions}</DialogActions>
    </Dialog>
  );
}
