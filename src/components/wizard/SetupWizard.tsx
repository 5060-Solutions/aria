import { useState, useCallback, useEffect, useRef } from "react";
import {
  Box,
  Button,
  TextField,
  MenuItem,
  Typography,
  alpha,
  useTheme,
  Collapse,
  IconButton,
  Tooltip,
  InputAdornment,
} from "@mui/material";
import CheckCircleOutlineIcon from "@mui/icons-material/CheckCircleOutline";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import ExpandLessIcon from "@mui/icons-material/ExpandLess";
import BugReportOutlinedIcon from "@mui/icons-material/BugReportOutlined";
import VisibilityIcon from "@mui/icons-material/Visibility";
import VisibilityOffIcon from "@mui/icons-material/VisibilityOff";
import { motion } from "framer-motion";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { useAppStore, saveAccountPassword } from "../../stores/appStore";
import { sipRegister } from "../../hooks/useSip";
import type { SipAccount, TransportType } from "../../types/sip";
import { log } from "../../utils/log";

const transports: { value: TransportType; labelKey: string }[] = [
  { value: "udp", labelKey: "wizard.transportUdp" },
  { value: "tcp", labelKey: "wizard.transportTcp" },
  { value: "tls", labelKey: "wizard.transportTls" },
];

const inputSx = {
  "& .MuiOutlinedInput-root": {
    borderRadius: "14px",
  },
};

export function SetupWizard() {
  const { t } = useTranslation();
  const theme = useTheme();
  const setAccount = useAppStore((s) => s.setAccount);
  const setSetupComplete = useAppStore((s) => s.setSetupComplete);

  const [showAdvanced, setShowAdvanced] = useState(false);
  const [showPassword, setShowPassword] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const waitingForReg = useRef(false);
  const [form, setForm] = useState({
    server: "",
    username: "",
    password: "",
    // Advanced
    displayName: "",
    authUsername: "",
    transport: "udp" as TransportType,
    port: 5060,
    registrar: "",
    outboundProxy: "",
  });

  const update = (field: string, value: string | number) => {
    setForm((f) => {
      const next = { ...f, [field]: value };
      if (field === "transport") {
        next.port = value === "tls" ? 5061 : 5060;
      }
      return next;
    });
  };

  const [error, setError] = useState<string | null>(null);
  const [regStatus, setRegStatus] = useState<string | null>(null);
  const canConnect = form.server && form.username && form.password;

  // Track if we've seen registering state (to distinguish initial unregistered from failed)
  const sawRegisteringState = useRef(false);
  // Track the account we're registering - only saved to store on success
  const registeringAccountId = useRef<string | null>(null);
  const pendingAccount = useRef<SipAccount | null>(null);

  // Watch registration state changes via subscription (not in effect body)
  useEffect(() => {
    return useAppStore.subscribe((state) => {
      if (!waitingForReg.current) return;

      // Check registration state using the account ID we're registering
      const targetId = registeringAccountId.current;
      if (!targetId) return;
      
      const targetAccountState = state.accountStates[targetId];
      const targetRegState = targetAccountState?.registrationState;

      log.info("[SetupWizard] Checking registration:", { targetId, targetRegState, sawRegistering: sawRegisteringState.current });

      if (targetRegState === "registering") {
        sawRegisteringState.current = true;
        setRegStatus(t("wizard.authenticating"));
      } else if (targetRegState === "registered" && pendingAccount.current) {
        // Guard: only process if we have a pending account (prevents re-entry)
        log.info("[SetupWizard] Registration successful! Saving account and transitioning.");
        
        // Clear refs FIRST to prevent re-entry when setAccount triggers store update
        const account = pendingAccount.current;
        pendingAccount.current = null;
        waitingForReg.current = false;
        sawRegisteringState.current = false;
        registeringAccountId.current = null;
        
        // Save password to secure storage
        saveAccountPassword(account.id, account.password);
        // Save account without password to store
        setAccount({ ...account, password: "" });
        
        setRegStatus(t("wizard.registered"));
        setConnecting(false);
        setSetupComplete(true);
      } else if (targetRegState === "error") {
        const errorMsg = targetAccountState?.registrationError;
        log.info("[SetupWizard] Registration error:", errorMsg);
        waitingForReg.current = false;
        sawRegisteringState.current = false;
        registeringAccountId.current = null;
        pendingAccount.current = null;
        setConnecting(false);
        setRegStatus(null);
        setError(errorMsg || t("wizard.registrationFailed"));
      } else if (targetRegState === "unregistered" && sawRegisteringState.current) {
        // Only handle unregistered if we were previously registering (meaning it failed/timed out)
        log.info("[SetupWizard] Registration reset after attempt");
        waitingForReg.current = false;
        sawRegisteringState.current = false;
        registeringAccountId.current = null;
        pendingAccount.current = null;
        setConnecting(false);
        setRegStatus(null);
      }
      // Ignore initial "unregistered" state - that's just the starting state
    });
  }, [setSetupComplete, setAccount, t]);

  // Frontend timeout: if still connecting after 35 seconds, reset
  // (longer than backend timeout of 30s to let backend handle it first)
  useEffect(() => {
    if (!connecting) return;
    const timeout = setTimeout(() => {
      if (waitingForReg.current) {
        log.info("[SetupWizard] Frontend timeout triggered");
        waitingForReg.current = false;
        registeringAccountId.current = null;
        setConnecting(false);
        setRegStatus(null);
        setError(t("wizard.connectionTimeout"));
      }
    }, 35000);
    return () => clearTimeout(timeout);
  }, [connecting, t]);

  const handleConnect = useCallback(async () => {
    if (!canConnect || connecting) return;
    
    // Prevent duplicate registration attempts
    if (registeringAccountId.current) {
      log.info("Registration already in progress for", registeringAccountId.current);
      return;
    }

    setConnecting(true);
    setError(null);
    waitingForReg.current = true;

    const accountId = crypto.randomUUID();
    registeringAccountId.current = accountId;
    
    const account: SipAccount = {
      id: accountId,
      displayName: form.displayName || form.username,
      username: form.username,
      domain: form.server,
      password: form.password,
      transport: form.transport,
      port: form.port,
      registrar: form.registrar || undefined,
      outboundProxy: form.outboundProxy || undefined,
      authUsername: form.authUsername || undefined,
      enabled: true,
    };

    // Store pending account - will be saved to store only on successful registration
    pendingAccount.current = account;

    try {
      // Use account with password for registration
      await sipRegister(account);
    } catch (e) {
      waitingForReg.current = false;
      registeringAccountId.current = null;
      pendingAccount.current = null;
      setError(String(e));
      setConnecting(false);
    }
  }, [canConnect, connecting, form]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && canConnect && !connecting) {
      e.preventDefault();
      handleConnect();
    }
  };

  return (
    <Box
      sx={{
        height: "100vh",
        display: "flex",
        flexDirection: "column",
        bgcolor: "background.default",
        overflow: "auto",
      }}
      onKeyDown={handleKeyDown}
    >
      {/* Developer diagnostics button — opens separate window */}
      <Box sx={{ position: "absolute", top: 8, right: 8, zIndex: 10 }}>
        <Tooltip title={t("wizard.developerDiagnostics")}>
          <IconButton
            size="small"
            onClick={() => invoke("open_debug_window").catch(() => {})}
            sx={{ opacity: 0.4, "&:hover": { opacity: 1 } }}
          >
            <BugReportOutlinedIcon fontSize="small" />
          </IconButton>
        </Tooltip>
      </Box>

      {/* Branding */}
      <Box sx={{ pt: 7, pb: 3, px: 3, textAlign: "center" }}>
        <motion.div
          initial={{ scale: 0.8, opacity: 0 }}
          animate={{ scale: 1, opacity: 1 }}
          transition={{ duration: 0.4, ease: "easeOut" }}
        >
          <Box
            sx={{
              mb: 2.5,
              display: "inline-block",
              filter: `drop-shadow(0 8px 32px ${alpha(theme.palette.primary.main, 0.3)})`,
            }}
          >
            <svg viewBox="0 0 512 512" width={72} height={72}>
              <defs>
                <linearGradient id="bgGrad" x1="0%" y1="0%" x2="100%" y2="100%">
                  <stop offset="0%" stopColor="#6366f1" />
                  <stop offset="50%" stopColor="#8b5cf6" />
                  <stop offset="100%" stopColor="#a855f7" />
                </linearGradient>
                <linearGradient id="waveGrad" x1="0%" y1="0%" x2="100%" y2="0%">
                  <stop offset="0%" stopColor="#ffffff" stopOpacity="0.9" />
                  <stop offset="100%" stopColor="#e0e7ff" stopOpacity="0.95" />
                </linearGradient>
              </defs>
              <rect x="32" y="32" width="448" height="448" rx="96" ry="96" fill="url(#bgGrad)" />
              <g transform="translate(256, 256)">
                <rect x="-16" y="-120" width="32" height="240" rx="16" fill="url(#waveGrad)" />
                <rect x="-72" y="-80" width="28" height="160" rx="14" fill="url(#waveGrad)" opacity="0.9" />
                <rect x="-120" y="-48" width="24" height="96" rx="12" fill="url(#waveGrad)" opacity="0.75" />
                <rect x="-160" y="-28" width="20" height="56" rx="10" fill="url(#waveGrad)" opacity="0.55" />
                <rect x="44" y="-80" width="28" height="160" rx="14" fill="url(#waveGrad)" opacity="0.9" />
                <rect x="96" y="-48" width="24" height="96" rx="12" fill="url(#waveGrad)" opacity="0.75" />
                <rect x="140" y="-28" width="20" height="56" rx="10" fill="url(#waveGrad)" opacity="0.55" />
              </g>
            </svg>
          </Box>
        </motion.div>
        <Typography
          variant="h4"
          sx={{ fontWeight: 600, letterSpacing: "-0.02em" }}
        >
          {t("wizard.branding")}
        </Typography>
        <Typography
          variant="body2"
          sx={{ color: "text.secondary", mt: 0.5 }}
        >
          {t("wizard.byCompany")}
        </Typography>
      </Box>

      {/* Form */}
      <motion.div
        initial={{ y: 20, opacity: 0 }}
        animate={{ y: 0, opacity: 1 }}
        transition={{ duration: 0.4, delay: 0.15 }}
      >
        <Box
          sx={{
            px: 3,
            display: "flex",
            flexDirection: "column",
            gap: 2,
          }}
        >
          <TextField
            label={t("wizard.server")}
            size="small"
            value={form.server}
            onChange={(e) => update("server", e.target.value)}
            placeholder={t("wizard.serverPlaceholder")}
            autoFocus
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            sx={inputSx}
          />
          <TextField
            label={t("wizard.username")}
            size="small"
            value={form.username}
            onChange={(e) => update("username", e.target.value)}
            placeholder={t("wizard.usernamePlaceholder")}
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            sx={inputSx}
          />
          <TextField
            label={t("wizard.password")}
            size="small"
            type={showPassword ? "text" : "password"}
            value={form.password}
            onChange={(e) => update("password", e.target.value)}
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            sx={inputSx}
            InputProps={{
              endAdornment: (
                <InputAdornment position="end">
                  <IconButton
                    size="small"
                    onClick={() => setShowPassword(!showPassword)}
                    edge="end"
                  >
                    {showPassword ? <VisibilityOffIcon fontSize="small" /> : <VisibilityIcon fontSize="small" />}
                  </IconButton>
                </InputAdornment>
              ),
            }}
          />

          {/* Advanced toggle */}
          <Button
            size="small"
            onClick={() => setShowAdvanced(!showAdvanced)}
            endIcon={showAdvanced ? <ExpandLessIcon /> : <ExpandMoreIcon />}
            sx={{
              alignSelf: "flex-start",
              color: "text.secondary",
              textTransform: "none",
              fontSize: "0.8rem",
              borderRadius: "12px",
              px: 1.5,
            }}
          >
            {t("wizard.advancedOptions")}
          </Button>

          <Collapse in={showAdvanced}>
            <Box sx={{ display: "flex", flexDirection: "column", gap: 2 }}>
              <TextField
                label={t("wizard.displayName")}
                size="small"
                value={form.displayName}
                onChange={(e) => update("displayName", e.target.value)}
                placeholder={t("wizard.displayNamePlaceholder")}
                spellCheck={false}
                sx={inputSx}
              />
              <TextField
                label={t("wizard.authUsername")}
                size="small"
                value={form.authUsername}
                onChange={(e) => update("authUsername", e.target.value)}
                placeholder={t("wizard.authUsernamePlaceholder")}
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                sx={inputSx}
              />
              <Box sx={{ display: "flex", gap: 1.5 }}>
                <TextField
                  label={t("wizard.transport")}
                  size="small"
                  select
                  value={form.transport}
                  onChange={(e) => update("transport", e.target.value)}
                  sx={{ ...inputSx, flex: 1 }}
                >
                  {transports.map((transport) => (
                    <MenuItem key={transport.value} value={transport.value}>
                      {t(transport.labelKey)}
                    </MenuItem>
                  ))}
                </TextField>
                <TextField
                  label={t("wizard.port")}
                  size="small"
                  type="number"
                  value={form.port}
                  onChange={(e) =>
                    update("port", parseInt(e.target.value) || 5060)
                  }
                  sx={{ ...inputSx, width: 90 }}
                />
              </Box>
              <TextField
                label={t("wizard.registrar")}
                size="small"
                value={form.registrar}
                onChange={(e) => update("registrar", e.target.value)}
                placeholder={t("wizard.registrarPlaceholder")}
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                sx={inputSx}
              />
              <TextField
                label={t("wizard.outboundProxy")}
                size="small"
                value={form.outboundProxy}
                onChange={(e) => update("outboundProxy", e.target.value)}
                placeholder={t("wizard.outboundProxyPlaceholder")}
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                sx={inputSx}
              />
            </Box>
          </Collapse>

          {error && (
            <Typography
              variant="body2"
              sx={{ color: "error.main", fontSize: "0.8rem", mt: 0.5 }}
            >
              {error}
            </Typography>
          )}

          {regStatus && !error && (
            <Typography
              variant="body2"
              sx={{ color: "info.main", fontSize: "0.8rem", mt: 0.5 }}
            >
              {regStatus}
            </Typography>
          )}

          {/* Connect button */}
          <Button
            variant="contained"
            size="large"
            disabled={!canConnect || connecting}
            onClick={handleConnect}
            endIcon={!connecting && <CheckCircleOutlineIcon />}
            sx={{
              mt: 1,
              borderRadius: "20px",
              py: 1.3,
              fontSize: "1rem",
            }}
          >
            {connecting ? t("wizard.connecting") : t("wizard.connect")}
          </Button>
        </Box>
      </motion.div>

      {/* Footer */}
      <Box sx={{ flex: 1 }} />
      <Box sx={{ textAlign: "center", pb: 3, pt: 2 }}>
        <Typography variant="caption" sx={{ color: "text.secondary" }}>
          {t("wizard.tagline")}
        </Typography>
      </Box>
    </Box>
  );
}

