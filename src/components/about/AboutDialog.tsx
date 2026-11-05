import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  Box,
  Button,
  ButtonBase,
  CircularProgress,
  Dialog,
  IconButton,
  LinearProgress,
  Tooltip,
  Typography,
  alpha,
  useTheme,
} from "@mui/material";
import CloseRoundedIcon from "@mui/icons-material/CloseRounded";
import ContentCopyRoundedIcon from "@mui/icons-material/ContentCopyRounded";
import CheckRoundedIcon from "@mui/icons-material/CheckRounded";
import CheckCircleRoundedIcon from "@mui/icons-material/CheckCircleRounded";
import ErrorOutlineRoundedIcon from "@mui/icons-material/ErrorOutlineRounded";
import NewReleasesRoundedIcon from "@mui/icons-material/NewReleasesRounded";
import SystemUpdateAltIcon from "@mui/icons-material/SystemUpdateAlt";
import LanguageRoundedIcon from "@mui/icons-material/LanguageRounded";
import ArticleOutlinedIcon from "@mui/icons-material/ArticleOutlined";
import GavelRoundedIcon from "@mui/icons-material/GavelRounded";
import VolunteerActivismOutlinedIcon from "@mui/icons-material/VolunteerActivismOutlined";
import OpenInNewRoundedIcon from "@mui/icons-material/OpenInNewRounded";
import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useAboutStore } from "../../stores/aboutStore";
import { useAppStore } from "../../stores/appStore";
import { canSelfUpdate, useUpdateStore } from "../../stores/updateStore";
import {
  ACKNOWLEDGEMENTS_URL,
  WEBSITE_URL,
  licenseUrl,
  platformLabel,
  releaseNotesUrl,
  versionDetails,
  type BuildInfo,
} from "../../utils/about";
import { log } from "../../utils/log";

/** The icon's own gradient (public/icon.svg), for the glow behind it. */
const ICON_GLOW = "#8b5cf6";

function openLink(url: string) {
  openUrl(url).catch((e) => log.error("[About] Could not open link:", e));
}

/**
 * The version and build, fetched once per opening. The update store also
 * learns the version, but only after a check has run.
 */
function useBuildDetails(active: boolean) {
  const [version, setVersion] = useState<string | null>(null);
  const [build, setBuild] = useState<BuildInfo | null>(null);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    getVersion()
      .then((v) => {
        if (!cancelled) setVersion(v);
      })
      .catch((e) => log.warn("[About] Could not read the version:", e));
    invoke<BuildInfo>("app_build_info")
      .then((b) => {
        if (!cancelled) setBuild(b);
      })
      .catch((e) => log.warn("[About] Could not read the build info:", e));
    return () => {
      cancelled = true;
    };
  }, [active]);

  return { version, build };
}

/** The version pill: shows version and commit, copies both on click. */
function VersionPill({ version, build }: { version: string; build: BuildInfo | null }) {
  const { t } = useTranslation();
  const theme = useTheme();
  const [copied, setCopied] = useState(false);
  const resetRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => () => {
    if (resetRef.current) clearTimeout(resetRef.current);
  }, []);

  const copy = () => {
    navigator.clipboard
      .writeText(versionDetails(version, build))
      .then(() => {
        setCopied(true);
        if (resetRef.current) clearTimeout(resetRef.current);
        resetRef.current = setTimeout(() => setCopied(false), 1600);
      })
      .catch((e) => log.warn("[About] Could not copy the version:", e));
  };

  return (
    <Tooltip title={copied ? t("about.copied") : t("about.copyDetails")} placement="bottom">
      <ButtonBase
        onClick={copy}
        aria-label={t("about.copyDetails")}
        sx={{
          gap: 0.75,
          px: 1.5,
          py: 0.5,
          borderRadius: "999px",
          border: `1px solid ${alpha(theme.palette.text.primary, 0.12)}`,
          bgcolor: alpha(theme.palette.background.paper, 0.6),
          color: "text.secondary",
          transition: "border-color 120ms ease, color 120ms ease",
          "&:hover": {
            color: "text.primary",
            borderColor: alpha(theme.palette.primary.main, 0.5),
          },
        }}
      >
        <Typography
          component="span"
          variant="caption"
          sx={{ fontWeight: 500, fontVariantNumeric: "tabular-nums", letterSpacing: "0.01em" }}
        >
          {t("about.version", { version })}
        </Typography>
        {build?.commit && (
          <Typography
            component="span"
            variant="caption"
            sx={{
              fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
              fontSize: "0.7rem",
              opacity: 0.75,
            }}
          >
            {build.commit}
          </Typography>
        )}
        {copied ? (
          <CheckRoundedIcon sx={{ fontSize: 14, color: "primary.main" }} />
        ) : (
          <ContentCopyRoundedIcon sx={{ fontSize: 13, opacity: 0.6 }} />
        )}
      </ButtonBase>
    </Tooltip>
  );
}

const actionSx = {
  borderRadius: "12px",
  px: 1.75,
  py: 0.5,
  fontSize: "0.8rem",
  minWidth: 0,
  whiteSpace: "nowrap",
} as const;

/**
 * The update check, inline. Covers every state the store can be in, so the
 * "Check for Updates…" menu item always has an answer to show, including the
 * "up to date" that the automatic checks keep to themselves.
 */
function UpdatePanel({ version }: { version: string | null }) {
  const { t } = useTranslation();
  const theme = useTheme();

  const checkStatus = useUpdateStore((s) => s.checkStatus);
  const installStatus = useUpdateStore((s) => s.installStatus);
  const availableVersion = useUpdateStore((s) => s.availableVersion);
  const currentVersion = useUpdateStore((s) => s.currentVersion);
  const errorMessage = useUpdateStore((s) => s.errorMessage);
  const downloadedBytes = useUpdateStore((s) => s.downloadedBytes);
  const totalBytes = useUpdateStore((s) => s.totalBytes);
  const checkForUpdate = useUpdateStore((s) => s.checkForUpdate);
  const installUpdate = useUpdateStore((s) => s.installUpdate);
  const restartApp = useUpdateStore((s) => s.restartApp);
  const openReleasePage = useUpdateStore((s) => s.openReleasePage);

  const inCall = useAppStore((s) =>
    s.activeCalls.some((c) => c.state !== "idle" && c.state !== "ended")
  );

  const busy = installStatus === "downloading" || installStatus === "installing";
  const installed = installStatus === "installed";
  const available = checkStatus === "available";
  const percent =
    totalBytes && totalBytes > 0
      ? Math.min(100, Math.round((downloadedBytes / totalBytes) * 100))
      : null;

  let icon: ReactNode;
  let title: string;
  let detail: string | null = null;
  let tone: "neutral" | "good" | "accent" | "error" = "neutral";
  let action: ReactNode = null;

  if (installed) {
    tone = "accent";
    icon = <CheckCircleRoundedIcon sx={{ fontSize: 22, color: "primary.main" }} />;
    title = t("update.installedTitle");
    detail = inCall ? t("about.restartAfterCall") : t("update.installedBody");
    action = (
      <Button variant="contained" size="small" disabled={inCall} onClick={() => void restartApp()} sx={actionSx}>
        {t("update.restartNow")}
      </Button>
    );
  } else if (available) {
    tone = installStatus === "error" ? "error" : "accent";
    icon = <NewReleasesRoundedIcon sx={{ fontSize: 22, color: "primary.main" }} />;
    title = t("update.available", { version: availableVersion ?? "" });
    detail =
      installStatus === "error"
        ? t("update.installFailed", { error: errorMessage ?? "" })
        : canSelfUpdate
          ? null
          : t("update.linuxNote");
    action = canSelfUpdate ? (
      <Button
        variant="contained"
        size="small"
        disabled={busy}
        onClick={() => void installUpdate()}
        sx={actionSx}
      >
        {busy ? (
          <CircularProgress size={16} color="inherit" />
        ) : installStatus === "error" ? (
          t("update.retry")
        ) : (
          t("update.install")
        )}
      </Button>
    ) : (
      <Button
        variant="contained"
        size="small"
        onClick={() => void openReleasePage()}
        endIcon={<OpenInNewRoundedIcon sx={{ fontSize: 16 }} />}
        sx={actionSx}
      >
        {t("update.openDownloads")}
      </Button>
    );
  } else if (checkStatus === "checking") {
    icon = <CircularProgress size={20} thickness={5} />;
    title = t("update.checking");
  } else if (checkStatus === "upToDate") {
    tone = "good";
    icon = <CheckCircleRoundedIcon sx={{ fontSize: 22, color: "success.main" }} />;
    title = t("update.upToDate", { version: currentVersion ?? version ?? "" });
  } else if (checkStatus === "error") {
    tone = "error";
    icon = <ErrorOutlineRoundedIcon sx={{ fontSize: 22, color: "error.main" }} />;
    title = t("about.checkFailedTitle");
    detail = t("about.checkFailedDetail", { error: errorMessage ?? "" });
  } else {
    icon = <SystemUpdateAltIcon sx={{ fontSize: 20, color: "text.secondary" }} />;
    title = t("update.checkForUpdates");
    detail = t("update.checkNow");
  }

  // Checking again is offered whenever nothing better is: idle, after an
  // answer, after a failure. Never while a check or an install is running.
  if (!action && checkStatus !== "checking") {
    action = (
      <Button
        variant="outlined"
        size="small"
        onClick={() => void checkForUpdate(true)}
        sx={actionSx}
      >
        {checkStatus === "error" ? t("update.retry") : t("about.checkNow")}
      </Button>
    );
  }

  const tint =
    tone === "accent"
      ? theme.palette.primary.main
      : tone === "error"
        ? theme.palette.error.main
        : tone === "good"
          ? theme.palette.success.main
          : theme.palette.text.primary;

  return (
    <Box
      role="status"
      aria-live="polite"
      sx={{
        p: 1.75,
        borderRadius: "16px",
        border: `1px solid ${alpha(tint, tone === "neutral" ? 0.08 : 0.22)}`,
        bgcolor: alpha(tint, tone === "neutral" ? 0.03 : 0.07),
        transition: "background-color 200ms ease, border-color 200ms ease",
      }}
    >
      <Box sx={{ display: "flex", alignItems: "center", gap: 1.5 }}>
        <Box sx={{ width: 24, display: "flex", justifyContent: "center", flexShrink: 0 }}>
          {icon}
        </Box>
        <Box sx={{ flex: 1, minWidth: 0 }}>
          <Typography variant="body2" sx={{ fontWeight: 500, lineHeight: 1.35 }}>
            {title}
          </Typography>
          {available && availableVersion && !busy && installStatus !== "error" && (
            <ButtonBase
              onClick={() => openLink(releaseNotesUrl(availableVersion))}
              sx={{
                mt: 0.25,
                borderRadius: "6px",
                color: "primary.main",
                typography: "caption",
                fontWeight: 500,
                "&:hover": { textDecoration: "underline" },
              }}
            >
              {t("about.whatsNew", { version: availableVersion })}
            </ButtonBase>
          )}
        </Box>
        {action && <Box sx={{ flexShrink: 0 }}>{action}</Box>}
      </Box>

      {detail && (
        <Typography
          variant="caption"
          sx={{
            display: "block",
            mt: 1,
            ml: 4.5,
            color: tone === "error" ? "error.main" : "text.secondary",
            wordBreak: "break-word",
          }}
        >
          {detail}
        </Typography>
      )}

      {busy && (
        <Box sx={{ mt: 1.25, ml: 4.5 }}>
          <Typography variant="caption" sx={{ color: "text.secondary", display: "block", mb: 0.5 }}>
            {installStatus === "installing"
              ? t("update.installing")
              : percent === null
                ? t("update.downloading")
                : t("update.downloadingPercent", { percent })}
          </Typography>
          <LinearProgress
            variant={installStatus === "downloading" && percent !== null ? "determinate" : "indeterminate"}
            value={percent ?? 0}
            sx={{ borderRadius: "8px", height: 5 }}
          />
        </Box>
      )}
    </Box>
  );
}

function LinkTile({ icon, label, onClick }: { icon: ReactNode; label: string; onClick: () => void }) {
  const theme = useTheme();
  return (
    <ButtonBase
      onClick={onClick}
      sx={{
        justifyContent: "flex-start",
        gap: 1,
        px: 1.25,
        py: 1.1,
        minHeight: 44,
        borderRadius: "14px",
        color: "text.primary",
        bgcolor: alpha(theme.palette.text.primary, 0.035),
        transition: "background-color 120ms ease",
        "&:hover": { bgcolor: alpha(theme.palette.primary.main, 0.1) },
        "& svg": { flexShrink: 0 },
      }}
    >
      <Box sx={{ display: "flex", color: "primary.main" }}>{icon}</Box>
      {/* Wraps rather than truncates if a translation runs long. */}
      <Typography
        variant="body2"
        sx={{
          flex: 1,
          minWidth: 0,
          textAlign: "left",
          fontSize: "0.8rem",
          fontWeight: 500,
          lineHeight: 1.2,
          overflowWrap: "break-word",
        }}
      >
        {label}
      </Typography>
    </ButtonBase>
  );
}

export function AboutDialog() {
  const { t } = useTranslation();
  const theme = useTheme();
  const isOpen = useAboutStore((s) => s.open);
  const closeAbout = useAboutStore((s) => s.closeAbout);
  const clearManualResult = useUpdateStore((s) => s.clearManualResult);
  const { version, build } = useBuildDetails(isOpen);

  const close = () => {
    closeAbout();
    const { checkStatus, installStatus, dismiss } = useUpdateStore.getState();
    // Closing an offered update is "Later", or the update prompt would pop up
    // the moment About goes. A download in progress is left alone: the prompt
    // taking over is how the user keeps track of it.
    if (checkStatus === "available" && (installStatus === "idle" || installStatus === "error")) {
      dismiss();
    }
    // An old "up to date" or error should not greet the next opening.
    clearManualResult();
  };

  const dark = theme.palette.mode === "dark";

  return (
    <Dialog
      open={isOpen}
      onClose={close}
      maxWidth="xs"
      fullWidth
      aria-labelledby="about-aria-title"
      slotProps={{
        paper: {
          sx: {
            borderRadius: "24px",
            overflow: "hidden",
            bgcolor: "background.default",
            backgroundImage: `radial-gradient(120% 60% at 50% 0%, ${alpha(ICON_GLOW, dark ? 0.2 : 0.12)} 0%, ${alpha(theme.palette.primary.main, dark ? 0.06 : 0.04)} 45%, transparent 75%)`,
          },
        },
      }}
    >
      <IconButton
        onClick={close}
        aria-label={t("common.close")}
        size="small"
        sx={{ position: "absolute", top: 12, right: 12, color: "text.secondary" }}
      >
        <CloseRoundedIcon sx={{ fontSize: 20 }} />
      </IconButton>

      <Box sx={{ px: 3, pt: 4.5, pb: 2.5, textAlign: "center" }}>
        <motion.div
          initial={{ scale: 0.86, opacity: 0, y: 6 }}
          animate={{ scale: 1, opacity: 1, y: 0 }}
          transition={{ type: "spring", stiffness: 260, damping: 22 }}
          style={{ display: "inline-block" }}
        >
          <Box
            component="img"
            src="/icon.png"
            alt=""
            draggable={false}
            sx={{
              width: 96,
              height: 96,
              display: "block",
              filter: `drop-shadow(0 14px 28px ${alpha(ICON_GLOW, dark ? 0.45 : 0.32)})`,
              userSelect: "none",
            }}
          />
        </motion.div>

        <Typography
          id="about-aria-title"
          variant="h4"
          sx={{ mt: 1.5, fontWeight: 600, letterSpacing: "-0.03em", lineHeight: 1.1 }}
        >
          Aria
        </Typography>
        <Typography variant="body2" sx={{ mt: 0.5, color: "text.secondary" }}>
          {t("about.tagline")}
        </Typography>

        <Box sx={{ mt: 2, minHeight: 28, display: "flex", justifyContent: "center" }}>
          {version && <VersionPill version={version} build={build} />}
        </Box>
        {build && (
          <Typography
            variant="caption"
            sx={{ display: "block", mt: 0.75, color: "text.secondary", opacity: 0.8 }}
          >
            {platformLabel(build)}
            {!build.release && ` · ${t("about.debugBuild")}`}
          </Typography>
        )}
      </Box>

      <Box sx={{ px: 2.5, pb: 1.5 }}>
        <UpdatePanel version={version} />
      </Box>

      <Box
        sx={{
          px: 2.5,
          pb: 2,
          display: "grid",
          gridTemplateColumns: "repeat(2, minmax(0, 1fr))",
          gap: 1,
        }}
      >
        <LinkTile
          icon={<LanguageRoundedIcon sx={{ fontSize: 18 }} />}
          label={t("about.website")}
          onClick={() => openLink(WEBSITE_URL)}
        />
        <LinkTile
          icon={<ArticleOutlinedIcon sx={{ fontSize: 18 }} />}
          label={t("about.releaseNotes")}
          onClick={() => version && openLink(releaseNotesUrl(version))}
        />
        <LinkTile
          icon={<GavelRoundedIcon sx={{ fontSize: 18 }} />}
          label={t("about.license")}
          onClick={() => openLink(licenseUrl(version))}
        />
        <LinkTile
          icon={<VolunteerActivismOutlinedIcon sx={{ fontSize: 18 }} />}
          label={t("about.acknowledgements")}
          onClick={() => openLink(ACKNOWLEDGEMENTS_URL)}
        />
      </Box>

      <Box
        sx={{
          px: 3,
          py: 1.75,
          textAlign: "center",
          borderTop: `1px solid ${alpha(theme.palette.text.primary, 0.06)}`,
        }}
      >
        <Typography variant="caption" sx={{ display: "block", color: "text.secondary", fontWeight: 500 }}>
          © 5060 Solutions
        </Typography>
        <Typography variant="caption" sx={{ display: "block", color: "text.secondary", opacity: 0.75 }}>
          {t("settings.aboutLicense")}
        </Typography>
      </Box>
    </Dialog>
  );
}
