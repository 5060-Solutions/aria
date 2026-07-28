import {
  Box,
  Button,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  LinearProgress,
  Typography,
  alpha,
  useTheme,
} from "@mui/material";
import SystemUpdateAltIcon from "@mui/icons-material/SystemUpdateAlt";
import OpenInNewIcon from "@mui/icons-material/OpenInNew";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { canSelfUpdate, useUpdateStore } from "../../stores/updateStore";

/**
 * The update prompt. Never installs on its own: a softphone that restarts
 * itself would drop a live call, so the user always decides — and while a call
 * is up the prompt is not even shown, it simply waits for the call to end.
 */
export function UpdateDialog() {
  const { t } = useTranslation();
  const theme = useTheme();

  const checkStatus = useUpdateStore((s) => s.checkStatus);
  const installStatus = useUpdateStore((s) => s.installStatus);
  const availableVersion = useUpdateStore((s) => s.availableVersion);
  const releaseNotes = useUpdateStore((s) => s.releaseNotes);
  const currentVersion = useUpdateStore((s) => s.currentVersion);
  const errorMessage = useUpdateStore((s) => s.errorMessage);
  const downloadedBytes = useUpdateStore((s) => s.downloadedBytes);
  const totalBytes = useUpdateStore((s) => s.totalBytes);
  const restartApp = useUpdateStore((s) => s.restartApp);
  const dismissed = useUpdateStore((s) => s.dismissed);
  const installUpdate = useUpdateStore((s) => s.installUpdate);
  const openReleasePage = useUpdateStore((s) => s.openReleasePage);
  const dismiss = useUpdateStore((s) => s.dismiss);

  // Same definition of "in a call" the shell uses to swap in the call screen.
  const inCall = useAppStore((s) =>
    s.activeCalls.some((c) => c.state !== "idle" && c.state !== "ended")
  );

  const busy = installStatus === "downloading" || installStatus === "installing";
  const finished = installStatus === "installed";
  // Once an install is under way we stay open even if a call starts, so the
  // user is never left wondering what happened to the download.
  const open =
    checkStatus === "available" &&
    !dismissed &&
    (!inCall || busy || finished || installStatus === "error");

  const percent =
    totalBytes && totalBytes > 0
      ? Math.min(100, Math.round((downloadedBytes / totalBytes) * 100))
      : null;

  return (
    <Dialog
      open={open}
      onClose={busy ? undefined : dismiss}
      maxWidth="xs"
      fullWidth
      slotProps={{
        paper: { sx: { borderRadius: "20px" } },
      }}
    >
      <DialogTitle sx={{ display: "flex", alignItems: "center", gap: 1.5 }}>
        <SystemUpdateAltIcon sx={{ fontSize: 22, color: "primary.main" }} />
        {finished ? t("update.installedTitle") : t("update.title")}
      </DialogTitle>

      <DialogContent>
        {finished ? (
          <Typography variant="body2" sx={{ color: "text.secondary" }}>
            {t("update.installedBody")}
          </Typography>
        ) : (
          <Box sx={{ display: "flex", flexDirection: "column", gap: 1.5 }}>
            <Box>
              <Typography variant="body2" sx={{ fontWeight: 500 }}>
                {t("update.available", { version: availableVersion ?? "" })}
              </Typography>
              {currentVersion && (
                <Typography variant="caption" sx={{ color: "text.secondary" }}>
                  {t("update.runningVersion", { version: currentVersion })}
                </Typography>
              )}
            </Box>

            {releaseNotes && (
              <Box>
                <Typography
                  variant="overline"
                  sx={{ color: "text.secondary", fontSize: "0.7rem" }}
                >
                  {t("update.releaseNotes")}
                </Typography>
                <Box
                  sx={{
                    mt: 0.5,
                    p: 1.5,
                    maxHeight: 200,
                    overflow: "auto",
                    borderRadius: "16px",
                    border: `1px solid ${alpha(theme.palette.divider, 0.12)}`,
                    bgcolor: alpha(theme.palette.action.hover, 0.04),
                  }}
                >
                  <Typography
                    variant="caption"
                    sx={{
                      color: "text.secondary",
                      whiteSpace: "pre-wrap",
                      display: "block",
                    }}
                  >
                    {releaseNotes}
                  </Typography>
                </Box>
              </Box>
            )}

            {!canSelfUpdate && (
              <Typography variant="caption" sx={{ color: "text.secondary" }}>
                {t("update.linuxNote")}
              </Typography>
            )}

            {busy && (
              <Box>
                <Typography
                  variant="caption"
                  sx={{ color: "text.secondary", display: "block", mb: 0.5 }}
                >
                  {installStatus === "installing"
                    ? t("update.installing")
                    : percent === null
                      ? t("update.downloading")
                      : t("update.downloadingPercent", { percent })}
                </Typography>
                <LinearProgress
                  variant={
                    installStatus === "downloading" && percent !== null
                      ? "determinate"
                      : "indeterminate"
                  }
                  value={percent ?? 0}
                  sx={{ borderRadius: "8px", height: 6 }}
                />
              </Box>
            )}

            {installStatus === "error" && (
              <Typography variant="caption" sx={{ color: "error.main" }}>
                {t("update.installFailed", { error: errorMessage ?? "" })}
              </Typography>
            )}
          </Box>
        )}
      </DialogContent>

      <DialogActions sx={{ px: 3, pb: 2 }}>
        {finished ? (
          <>
            <Button onClick={dismiss} sx={{ borderRadius: "12px" }}>
              {t("update.later")}
            </Button>
            <Button
              variant="contained"
              onClick={restartApp}
              sx={{ borderRadius: "12px" }}
            >
              {t("update.restartNow")}
            </Button>
          </>
        ) : (
          <>
            <Button onClick={dismiss} disabled={busy} sx={{ borderRadius: "12px" }}>
              {t("update.later")}
            </Button>
            {canSelfUpdate ? (
              <Button
                variant="contained"
                onClick={() => void installUpdate()}
                disabled={busy}
                sx={{ borderRadius: "12px" }}
              >
                {busy ? (
                  <CircularProgress size={18} color="inherit" />
                ) : installStatus === "error" ? (
                  t("update.retry")
                ) : (
                  t("update.install")
                )}
              </Button>
            ) : (
              <Button
                variant="contained"
                onClick={() => void openReleasePage()}
                startIcon={<OpenInNewIcon sx={{ fontSize: 18 }} />}
                sx={{ borderRadius: "12px" }}
              >
                {t("update.openDownloads")}
              </Button>
            )}
          </>
        )}
      </DialogActions>
    </Dialog>
  );
}
