import { useState, useEffect, useMemo } from "react";
import {
  Box,
  Typography,
  Switch,
  Divider,
  alpha,
  useTheme,
  ListItemButton,
  ListItemText,
  ListItemIcon,
  List,
  Collapse,
  TextField,
  MenuItem,
  Button,
  CircularProgress,
  IconButton,
  Dialog,
  DialogTitle,
  DialogContent,
  DialogActions,
  Select,
  FormControl,
  InputLabel,
  Chip,
} from "@mui/material";
import DarkModeIcon from "@mui/icons-material/DarkMode";
import MicIcon from "@mui/icons-material/Mic";
import VolumeUpIcon from "@mui/icons-material/VolumeUp";
import AccountCircleIcon from "@mui/icons-material/AccountCircle";
import LogoutIcon from "@mui/icons-material/Logout";
import InfoOutlinedIcon from "@mui/icons-material/InfoOutlined";
import BugReportOutlinedIcon from "@mui/icons-material/BugReportOutlined";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import ExpandLessIcon from "@mui/icons-material/ExpandLess";
import AddIcon from "@mui/icons-material/Add";
import EditIcon from "@mui/icons-material/Edit";
import DeleteIcon from "@mui/icons-material/Delete";
import CheckCircleIcon from "@mui/icons-material/CheckCircle";
import RadioButtonUncheckedIcon from "@mui/icons-material/RadioButtonUnchecked";
import VisibilityIcon from "@mui/icons-material/Visibility";
import VisibilityOffIcon from "@mui/icons-material/VisibilityOff";
import MoreVertIcon from "@mui/icons-material/MoreVert";
import PowerSettingsNewIcon from "@mui/icons-material/PowerSettingsNew";
import FolderOpenIcon from "@mui/icons-material/FolderOpen";
import FiberManualRecordIcon from "@mui/icons-material/FiberManualRecord";
import ContactsIcon from "@mui/icons-material/Contacts";
import SyncIcon from "@mui/icons-material/Sync";
import CloudIcon from "@mui/icons-material/Cloud";
import PhoneIphoneIcon from "@mui/icons-material/PhoneIphone";
import LanguageIcon from "@mui/icons-material/Language";
import Menu from "@mui/material/Menu";
import InputAdornment from "@mui/material/InputAdornment";
import { useTranslation } from "react-i18next";
import { openRecordingsFolder, getDefaultRecordingsDir, fetchSystemContacts } from "../../hooks/useSip";
import { changeLanguage, supportedLanguages, getCurrentLanguage } from "../../i18n";
import { AudioLevelMeter } from "../call/AudioLevelMeter";
import { log } from "../../utils/log";

interface AudioDevice {
  name: string;
  isDefault: boolean;
}

interface AudioDevices {
  inputDevices: AudioDevice[];
  outputDevices: AudioDevice[];
}
import { invoke } from "@tauri-apps/api/core";
import { useAppStore, saveAccountPassword, loadAccountPassword, deleteAccountPassword, getAccountWithPassword } from "../../stores/appStore";
import { sipRegister, sipUnregisterAccount, sipSetActiveAccount } from "../../hooks/useSip";
import type { SipAccount, TransportType, CodecConfig } from "../../types/sip";
import { DEFAULT_CODECS, CODEC_INFO } from "../../types/sip";
import ArrowUpwardIcon from "@mui/icons-material/ArrowUpward";
import ArrowDownwardIcon from "@mui/icons-material/ArrowDownward";

const transports: { value: TransportType; labelKey: string }[] = [
  { value: "udp", labelKey: "wizard.transportUdp" },
  { value: "tcp", labelKey: "wizard.transportTcp" },
  { value: "tls", labelKey: "wizard.transportTls" },
];

const inputSx = {
  "& .MuiOutlinedInput-root": {
    borderRadius: "12px",
  },
};

interface AccountFormData {
  server: string;
  username: string;
  password: string;
  displayName: string;
  authUsername: string;
  authRealm: string;
  transport: TransportType;
  port: number;
  registrar: string;
  outboundProxy: string;
  autoRecord: boolean;
  srtpMode: "disabled" | "sdes" | "dtls";
  codecs: CodecConfig[];
}

const emptyForm: AccountFormData = {
  server: "",
  username: "",
  password: "",
  displayName: "",
  authUsername: "",
  authRealm: "",
  transport: "udp",
  port: 5060,
  registrar: "",
  outboundProxy: "",
  autoRecord: false,
  srtpMode: "disabled",
  codecs: [...DEFAULT_CODECS],
};

export function Settings() {
  const { t, i18n } = useTranslation();
  const darkMode = useAppStore((s) => s.darkMode);
  const toggleDarkMode = useAppStore((s) => s.toggleDarkMode);
  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const accountStates = useAppStore((s) => s.accountStates);
  const addAccount = useAppStore((s) => s.addAccount);
  const updateAccount = useAppStore((s) => s.updateAccount);
  const removeAccount = useAppStore((s) => s.removeAccount);
  const setActiveAccount = useAppStore((s) => s.setActiveAccount);
  const setAccountRegistrationState = useAppStore(
    (s) => s.setAccountRegistrationState
  );
  const setSetupComplete = useAppStore((s) => s.setSetupComplete);
  const theme = useTheme();
  const [selectedLanguage, setSelectedLanguage] = useState(getCurrentLanguage());

  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingAccount, setEditingAccount] = useState<SipAccount | null>(null);
  const [saving, setSaving] = useState(false);
  const [showAdvancedInDialog, setShowAdvancedInDialog] = useState(false);
  const [form, setForm] = useState<AccountFormData>(emptyForm);
  const [showPassword, setShowPassword] = useState(false);
  const [menuAnchor, setMenuAnchor] = useState<null | HTMLElement>(null);
  const [menuAccountId, setMenuAccountId] = useState<string | null>(null);
  const [audioDevices, setAudioDevices] = useState<AudioDevices | null>(null);
  const storeInputDevice = useAppStore((s) => s.selectedInputDevice);
  const storeOutputDevice = useAppStore((s) => s.selectedOutputDevice);
  const storeSetInputDevice = useAppStore((s) => s.setSelectedInputDevice);
  const storeSetOutputDevice = useAppStore((s) => s.setSelectedOutputDevice);
  const [selectedInputDevice, setSelectedInputDeviceLocal] = useState<string>(storeInputDevice ?? "default");
  const [selectedOutputDevice, setSelectedOutputDeviceLocal] = useState<string>(storeOutputDevice ?? "default");
  const [showAudioSettings, setShowAudioSettings] = useState(false);
  const [showContactsSettings, setShowContactsSettings] = useState(false);
  const [micTesting, setMicTesting] = useState(false);
  const [tonePlaying, setTonePlaying] = useState(false);
  const [recordingsDir, setRecordingsDir] = useState<string>("");
  const [syncingSystemContacts, setSyncingSystemContacts] = useState(false);

  // Contacts sync state from store
  const googleConnected = useAppStore((s) => s.googleConnected);
  const googleLastSync = useAppStore((s) => s.googleLastSync);
  const systemContactsEnabled = useAppStore((s) => s.systemContactsEnabled);
  const systemLastSync = useAppStore((s) => s.systemLastSync);
  const setSystemContactsEnabled = useAppStore((s) => s.setSystemContactsEnabled);
  const setSystemLastSync = useAppStore((s) => s.setSystemLastSync);
  const importContacts = useAppStore((s) => s.importContacts);
  const removeContactsBySource = useAppStore((s) => s.removeContactsBySource);
  const contacts = useAppStore((s) => s.contacts);

  // Load audio devices and recordings dir on mount
  useEffect(() => {
    invoke<AudioDevices>("get_audio_devices")
      .then((devices) => {
        setAudioDevices(devices);
        // If no stored preference, use the system default
        if (!storeInputDevice) {
          const defaultInput = devices.inputDevices.find(d => d.isDefault);
          if (defaultInput) setSelectedInputDeviceLocal(defaultInput.name);
        }
        if (!storeOutputDevice) {
          const defaultOutput = devices.outputDevices.find(d => d.isDefault);
          if (defaultOutput) setSelectedOutputDeviceLocal(defaultOutput.name);
        }
      })
      .catch((e) => log.error("Failed to load audio devices:", e));

    getDefaultRecordingsDir()
      .then(setRecordingsDir)
      .catch((e) => log.error("Failed to get recordings dir:", e));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (editingAccount) {
      // Load password from secure storage when editing
      loadAccountPassword(editingAccount.id).then((password) => {
        setForm({
          server: editingAccount.domain,
          username: editingAccount.username,
          password: password ?? "",
          displayName: editingAccount.displayName || "",
          authUsername: editingAccount.authUsername || "",
          authRealm: editingAccount.authRealm || "",
          transport: editingAccount.transport || "udp",
          port: editingAccount.port || 5060,
          registrar: editingAccount.registrar || "",
          outboundProxy: editingAccount.outboundProxy || "",
          autoRecord: editingAccount.autoRecord ?? false,
          srtpMode: editingAccount.srtpMode ?? "disabled",
          codecs: editingAccount.codecs ?? [...DEFAULT_CODECS],
        });
      });
    }
  }, [editingAccount]);

  const resetForm = () => {
    setForm(emptyForm);
    setShowAdvancedInDialog(false);
    setShowPassword(false);
  };

  const handleOpenMenu = (event: React.MouseEvent<HTMLElement>, accountId: string) => {
    event.stopPropagation();
    setMenuAnchor(event.currentTarget);
    setMenuAccountId(accountId);
  };

  const handleCloseMenu = () => {
    setMenuAnchor(null);
    setMenuAccountId(null);
  };

  const handleToggleAccountEnabled = async (accountId: string) => {
    const account = accounts.find((a) => a.id === accountId);
    if (!account) return;

    const newEnabled = !account.enabled;
    updateAccount({ ...account, enabled: newEnabled });

    if (!newEnabled) {
      // Disabling: unregister this specific account
      await sipUnregisterAccount(accountId).catch(() => {});
      setAccountRegistrationState(accountId, "unregistered");
    } else {
      // Enabling: register this account
      setAccountRegistrationState(accountId, "registering");
      try {
        const accountWithPassword = await getAccountWithPassword(account);
        await sipRegister(accountWithPassword);
      } catch (e) {
        setAccountRegistrationState(accountId, "error", String(e));
      }
    }

    handleCloseMenu();
  };

  const update = (field: keyof AccountFormData, value: string | number) => {
    setForm((f) => {
      const next = { ...f, [field]: value };
      if (field === "transport") {
        next.port = value === "tls" ? 5061 : 5060;
      }
      return next;
    });
  };

  const handleOpenAddDialog = () => {
    setEditingAccount(null);
    resetForm();
    setDialogOpen(true);
  };

  const handleOpenEditDialog = (account: SipAccount) => {
    setEditingAccount(account);
    setDialogOpen(true);
  };

  const handleCloseDialog = () => {
    setDialogOpen(false);
    setEditingAccount(null);
    resetForm();
    setShowPassword(false);
  };

  const canSave = form.server && form.username && form.password;

  const handleSaveAccount = async () => {
    if (!canSave) return;
    setSaving(true);

    const accountData: SipAccount = {
      id: editingAccount?.id ?? crypto.randomUUID(),
      displayName: form.displayName || form.username,
      username: form.username,
      domain: form.server,
      password: form.password,
      transport: form.transport,
      port: form.port,
      registrar: form.registrar || undefined,
      outboundProxy: form.outboundProxy || undefined,
      authUsername: form.authUsername || undefined,
      authRealm: form.authRealm || undefined,
      enabled: editingAccount?.enabled ?? true,
      autoRecord: form.autoRecord,
      srtpMode: form.srtpMode,
      codecs: form.codecs,
    };

    // Save password to secure storage (OS keychain)
    if (form.password) {
      await saveAccountPassword(accountData.id, form.password);
    }

    // Store account without password in local state
    const accountForStore = { ...accountData, password: "" };
    if (editingAccount) {
      updateAccount(accountForStore);
    } else {
      addAccount(accountForStore);
    }

    // If this is the first account, set it as active
    if (accounts.length === 0) {
      setActiveAccount(accountData.id);
    }

    // Register this account if enabled
    if (accountData.enabled) {
      setAccountRegistrationState(accountData.id, "registering");
      try {
        // Use accountData which still has the password for registration
        await sipRegister(accountData);
        // Also set it active in the backend
        await sipSetActiveAccount(accountData.id).catch(() => {});
      } catch (e) {
        setAccountRegistrationState(accountData.id, "error", String(e));
      }
    }

    setSaving(false);
    handleCloseDialog();
  };

  const handleSwitchAccount = async (accountId: string) => {
    if (accountId === activeAccountId) return;

    const account = accounts.find((a) => a.id === accountId);
    if (!account) return;

    // Just switch the active account - don't unregister/register
    setActiveAccount(accountId);

    // Also update the backend's active account
    await sipSetActiveAccount(accountId).catch(() => {});
  };

  const handleReconnect = async (accountId: string) => {
    const account = accounts.find((a) => a.id === accountId);
    if (!account) return;

    const currentState = getAccountStatus(accountId);
    if (currentState === "registered" || currentState === "registering" || currentState === "reconnecting") return;

    // Try to re-register just this account
    setAccountRegistrationState(accountId, "registering");

    try {
      const accountWithPassword = await getAccountWithPassword(account);
      await sipRegister(accountWithPassword);
    } catch (e) {
      setAccountRegistrationState(accountId, "error", String(e));
    }
  };

  const handleDeleteAccount = async (accountId: string) => {
    // Unregister this specific account
    await sipUnregisterAccount(accountId).catch(() => {});
    removeAccount(accountId);

    // Delete password from secure storage
    await deleteAccountPassword(accountId).catch(() => {});

    // If no accounts left, go back to setup
    if (accounts.length <= 1) {
      localStorage.removeItem("aria_setup_complete");
      setSetupComplete(false);
    }
  };

  const handleSignOutAll = async () => {
    // Unregister all accounts
    for (const account of accounts) {
      await sipUnregisterAccount(account.id).catch(() => {});
      await deleteAccountPassword(account.id).catch(() => {});
    }
    localStorage.removeItem("aria_accounts_v2");
    localStorage.removeItem("aria_setup_complete");
    setSetupComplete(false);
  };

  const getAccountStatus = (accountId: string) => {
    const state = accountStates[accountId];
    if (!state) return "unregistered";
    return state.registrationState;
  };

  const handleSyncSystemContacts = async () => {
    setSyncingSystemContacts(true);
    try {
      const systemContacts = await fetchSystemContacts();
      const mappedContacts = systemContacts
        .filter((c): c is typeof c & { phone: string } => c.phone != null)
        .map((c) => ({
          id: `system-${c.id}`,
          name: c.name,
          uri: `sip:${c.phone.replace(/\D/g, "")}`,
          phone: c.phone,
          favorite: false,
          source: "system" as const,
          sourceId: c.id,
        }));
      importContacts(mappedContacts, "system");
      setSystemLastSync(Date.now());
    } catch (e) {
      log.error("Failed to sync system contacts:", e);
    } finally {
      setSyncingSystemContacts(false);
    }
  };

  const handleToggleSystemContacts = async (enabled: boolean) => {
    setSystemContactsEnabled(enabled);
    if (enabled) {
      await handleSyncSystemContacts();
    } else {
      removeContactsBySource("system");
      setSystemLastSync(null);
    }
  };

  const dateTimeFormatter = useMemo(() => {
    return new Intl.DateTimeFormat(i18n.language, {
      month: "short",
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
    });
  }, [i18n.language]);
  
  const formatLastSync = (timestamp: number | null) => {
    if (!timestamp) return t("common.never");
    return dateTimeFormatter.format(timestamp);
  };
  
  const handleLanguageChange = (lang: string) => {
    setSelectedLanguage(lang);
    changeLanguage(lang);
  };

  // Stop mic test when audio settings collapse or component unmounts
  useEffect(() => {
    return () => {
      invoke("stop_audio_test").catch(() => {});
    };
  }, []);

  useEffect(() => {
    if (!showAudioSettings && micTesting) {
      invoke("stop_audio_test").catch(() => {});
      setMicTesting(false);
    }
  }, [showAudioSettings, micTesting]);

  const handleToggleMicTest = async () => {
    if (micTesting) {
      await invoke("stop_audio_test").catch(() => {});
      setMicTesting(false);
    } else {
      try {
        await invoke("start_audio_test", {
          deviceName: selectedInputDevice === "default" ? null : selectedInputDevice,
        });
        setMicTesting(true);
      } catch (e) {
        log.error("Failed to start mic test:", e);
      }
    }
  };

  const handlePlayTestTone = async () => {
    if (tonePlaying) return;
    setTonePlaying(true);
    try {
      await invoke("play_test_tone", {
        deviceName: selectedOutputDevice === "default" ? null : selectedOutputDevice,
      });
      // Tone plays for ~1.5s, reset button state after
      setTimeout(() => setTonePlaying(false), 2000);
    } catch (e) {
      log.error("Failed to play test tone:", e);
      setTonePlaying(false);
    }
  };

  const systemContactCount = contacts.filter((c) => c.source === "system").length;
  const googleContactCount = contacts.filter((c) => c.source === "google").length;

  return (
    <Box sx={{ height: "100%", overflow: "auto" }}>
      <Box sx={{ px: 2.5, pt: 2.5, pb: 1.5 }}>
        <Typography variant="h5" sx={{ fontWeight: 500 }}>
          {t("settings.title")}
        </Typography>
      </Box>

      <Box sx={{ px: 1.5 }}>
        {/* Accounts section */}
        <Box
          sx={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            px: 1,
            mb: 1,
          }}
        >
          <Typography
            variant="overline"
            sx={{ color: "text.secondary", fontSize: "0.7rem" }}
          >
            {t("settings.accounts")}
          </Typography>
          <IconButton size="small" onClick={handleOpenAddDialog}>
            <AddIcon sx={{ fontSize: 18 }} />
          </IconButton>
        </Box>

        {/* Account list - only show enabled accounts by default */}
        {accounts.filter(a => a.enabled).map((account) => {
          const isActive = account.id === activeAccountId;
          const status = getAccountStatus(account.id);

          return (
            <Box
              key={account.id}
              sx={{
                mx: 0.5,
                p: 1.5,
                borderRadius: "16px",
                bgcolor: isActive
                  ? alpha(theme.palette.primary.main, 0.08)
                  : "transparent",
                border: `1px solid ${
                  isActive
                    ? alpha(theme.palette.primary.main, 0.2)
                    : alpha(theme.palette.divider, 0.12)
                }`,
                mb: 1,
                cursor: "pointer",
                transition: "all 0.15s ease",
                "&:hover": {
                  bgcolor: isActive
                    ? alpha(theme.palette.primary.main, 0.12)
                    : alpha(theme.palette.action.hover, 0.04),
                },
              }}
              onClick={() => {
                if (isActive && (status === "error" || status === "unregistered")) {
                  handleReconnect(account.id);
                } else {
                  handleSwitchAccount(account.id);
                }
              }}
            >
              <Box sx={{ display: "flex", alignItems: "center", gap: 1.5 }}>
                {isActive ? (
                  <CheckCircleIcon
                    sx={{ color: "primary.main", fontSize: 20 }}
                  />
                ) : (
                  <RadioButtonUncheckedIcon
                    sx={{ color: "text.disabled", fontSize: 20 }}
                  />
                )}
                <AccountCircleIcon
                  sx={{
                    color: isActive ? "primary.main" : "text.secondary",
                    fontSize: 32,
                  }}
                />
                <Box sx={{ flex: 1, minWidth: 0 }}>
                  <Typography
                    variant="body2"
                    sx={{
                      fontWeight: isActive ? 600 : 500,
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                    }}
                  >
                    {account.displayName}
                  </Typography>
                  <Typography
                    variant="caption"
                    sx={{
                      color: "text.secondary",
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                      display: "block",
                    }}
                  >
                    {account.username}@{account.domain} · {account.transport.toUpperCase()}:{account.port}
                  </Typography>
                </Box>
                {isActive && (status === "error" || status === "unregistered") ? (
                  <Button
                    size="small"
                    variant="outlined"
                    color={status === "error" ? "error" : "primary"}
                    onClick={(e) => {
                      e.stopPropagation();
                      handleReconnect(account.id);
                    }}
                    sx={{
                      minWidth: "auto",
                      px: 1,
                      py: 0.25,
                      fontSize: "0.65rem",
                      borderRadius: "6px",
                    }}
                  >
                    {t("settings.reconnect")}
                  </Button>
                ) : (
                  <Box
                    sx={{
                      px: 0.75,
                      py: 0.25,
                      borderRadius: "6px",
                      bgcolor:
                        status === "registered"
                          ? alpha(theme.palette.success.main, 0.15)
                          : status === "registering" || status === "reconnecting"
                            ? alpha(theme.palette.warning.main, 0.15)
                            : status === "error"
                              ? alpha(theme.palette.error.main, 0.15)
                              : alpha(theme.palette.grey[500], 0.15),
                    }}
                  >
                    <Typography
                      variant="caption"
                      sx={{
                        fontWeight: 500,
                        fontSize: "0.6rem",
                        color:
                          status === "registered"
                            ? "success.main"
                            : status === "registering" || status === "reconnecting"
                              ? "warning.main"
                              : status === "error"
                                ? "error.main"
                                : "text.secondary",
                      }}
                    >
                      {status === "registered"
                        ? t("settings.online")
                        : status === "registering"
                          ? "..."
                          : status === "reconnecting"
                            ? t("settings.reconnect") + "..."
                            : status === "error"
                              ? t("settings.error")
                              : t("settings.offline")}
                    </Typography>
                  </Box>
                )}
                <IconButton
                  size="small"
                  onClick={(e) => handleOpenMenu(e, account.id)}
                  sx={{ ml: 0.5 }}
                >
                  <MoreVertIcon sx={{ fontSize: 18 }} />
                </IconButton>
              </Box>
            </Box>
          );
        })}

        {/* Disabled accounts section */}
        {accounts.filter(a => !a.enabled).length > 0 && (
          <>
            <Typography
              variant="overline"
              sx={{ color: "text.disabled", fontSize: "0.65rem", px: 1, mt: 1, display: "block" }}
            >
              {t("settings.disabled")}
            </Typography>
            {accounts.filter(a => !a.enabled).map((account) => (
              <Box
                key={account.id}
                sx={{
                  mx: 0.5,
                  p: 1.5,
                  borderRadius: "16px",
                  bgcolor: alpha(theme.palette.action.disabled, 0.04),
                  border: `1px solid ${alpha(theme.palette.divider, 0.08)}`,
                  mb: 1,
                  opacity: 0.6,
                }}
              >
                <Box sx={{ display: "flex", alignItems: "center", gap: 1.5 }}>
                  <RadioButtonUncheckedIcon
                    sx={{ color: "text.disabled", fontSize: 20 }}
                  />
                  <AccountCircleIcon
                    sx={{ color: "text.disabled", fontSize: 32 }}
                  />
                  <Box sx={{ flex: 1, minWidth: 0 }}>
                    <Typography
                      variant="body2"
                      sx={{
                        fontWeight: 500,
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                        whiteSpace: "nowrap",
                        color: "text.disabled",
                      }}
                    >
                      {account.displayName}
                    </Typography>
                    <Typography
                      variant="caption"
                      sx={{
                        color: "text.disabled",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                        whiteSpace: "nowrap",
                        display: "block",
                      }}
                    >
                      {account.username}@{account.domain}
                    </Typography>
                  </Box>
                  <IconButton
                    size="small"
                    onClick={(e) => handleOpenMenu(e, account.id)}
                  >
                    <MoreVertIcon sx={{ fontSize: 18 }} />
                  </IconButton>
                </Box>
              </Box>
            ))}
          </>
        )}

        {/* Account context menu */}
        <Menu
          anchorEl={menuAnchor}
          open={Boolean(menuAnchor)}
          onClose={handleCloseMenu}
          anchorOrigin={{ vertical: "bottom", horizontal: "right" }}
          transformOrigin={{ vertical: "top", horizontal: "right" }}
          PaperProps={{ sx: { borderRadius: "12px", minWidth: 160 } }}
        >
          <MenuItem
            onClick={() => {
              const account = accounts.find(a => a.id === menuAccountId);
              if (account) handleOpenEditDialog(account);
              handleCloseMenu();
            }}
          >
            <ListItemIcon><EditIcon fontSize="small" /></ListItemIcon>
            <ListItemText>{t("settings.edit")}</ListItemText>
          </MenuItem>
          <MenuItem onClick={() => menuAccountId && handleToggleAccountEnabled(menuAccountId)}>
            <ListItemIcon>
              <PowerSettingsNewIcon fontSize="small" />
            </ListItemIcon>
            <ListItemText>
              {accounts.find(a => a.id === menuAccountId)?.enabled ? t("settings.disable") : t("settings.enable")}
            </ListItemText>
          </MenuItem>
          <Divider />
          <MenuItem
            onClick={() => {
              if (menuAccountId) handleDeleteAccount(menuAccountId);
              handleCloseMenu();
            }}
            sx={{ color: "error.main" }}
          >
            <ListItemIcon><DeleteIcon fontSize="small" sx={{ color: "error.main" }} /></ListItemIcon>
            <ListItemText>{t("settings.delete")}</ListItemText>
          </MenuItem>
        </Menu>

        {accounts.length === 0 && (
          <Box sx={{ textAlign: "center", py: 3 }}>
            <Typography variant="body2" sx={{ color: "text.secondary" }}>
              {t("settings.noAccounts")}
            </Typography>
            <Button
              size="small"
              startIcon={<AddIcon />}
              onClick={handleOpenAddDialog}
              sx={{ mt: 1 }}
            >
              {t("settings.addAccount")}
            </Button>
          </Box>
        )}


        <Divider sx={{ my: 1.5, opacity: 0.15 }} />

        {/* Preferences */}
        <List disablePadding>
          <ListItemButton
            onClick={toggleDarkMode}
            sx={{ borderRadius: "16px", mb: 0.5 }}
          >
            <ListItemIcon sx={{ minWidth: 40 }}>
              <DarkModeIcon sx={{ fontSize: 20 }} />
            </ListItemIcon>
            <ListItemText
              primary={t("settings.darkMode")}
              primaryTypographyProps={{ fontSize: "0.9rem" }}
            />
            <Switch
              edge="end"
              checked={darkMode}
              onChange={toggleDarkMode}
              size="small"
            />
          </ListItemButton>
          
          {/* Language selector */}
          <ListItemButton
            sx={{ borderRadius: "16px", mb: 0.5 }}
          >
            <ListItemIcon sx={{ minWidth: 40 }}>
              <LanguageIcon sx={{ fontSize: 20 }} />
            </ListItemIcon>
            <ListItemText
              primary={t("settings.language")}
              primaryTypographyProps={{ fontSize: "0.9rem" }}
            />
            <Select
              value={selectedLanguage}
              onChange={(e) => handleLanguageChange(e.target.value)}
              size="small"
              variant="outlined"
              sx={{
                minWidth: 120,
                "& .MuiOutlinedInput-notchedOutline": { border: "none" },
                "& .MuiSelect-select": { py: 0.5, fontSize: "0.85rem" },
              }}
            >
              {supportedLanguages.map((lang) => (
                <MenuItem key={lang.code} value={lang.code}>
                  {lang.nativeName}
                </MenuItem>
              ))}
            </Select>
          </ListItemButton>

          {/* Audio devices section */}
          <ListItemButton
            onClick={() => setShowAudioSettings(!showAudioSettings)}
            sx={{ borderRadius: "16px", mb: 0.5 }}
          >
            <ListItemIcon sx={{ minWidth: 40 }}>
              <VolumeUpIcon sx={{ fontSize: 20 }} />
            </ListItemIcon>
            <ListItemText
              primary={t("settings.audioDevices")}
              secondary={audioDevices ? t("settings.inputsOutputs", { inputs: audioDevices.inputDevices.length, outputs: audioDevices.outputDevices.length }) : t("common.loading")}
              primaryTypographyProps={{ fontSize: "0.9rem" }}
              secondaryTypographyProps={{ fontSize: "0.7rem" }}
            />
            {showAudioSettings ? <ExpandLessIcon sx={{ fontSize: 20 }} /> : <ExpandMoreIcon sx={{ fontSize: 20 }} />}
          </ListItemButton>

          <Collapse in={showAudioSettings}>
            <Box sx={{ px: 2, pb: 2 }}>
              {audioDevices && (
                <>
                  <Box sx={{ display: "flex", gap: 1, alignItems: "flex-end", mb: 2 }}>
                    <FormControl fullWidth size="small">
                      <InputLabel>{t("settings.microphone")}</InputLabel>
                      <Select
                        value={selectedInputDevice}
                        label={t("settings.microphone")}
                        onChange={(e) => {
                          const val = e.target.value;
                          setSelectedInputDeviceLocal(val);
                          storeSetInputDevice(val);
                          invoke("set_audio_devices", {
                            inputDevice: val === "default" ? null : val,
                            outputDevice: selectedOutputDevice === "default" ? null : selectedOutputDevice,
                          }).catch(() => {});
                          // Restart mic test with new device if testing
                          if (micTesting) {
                            invoke("stop_audio_test").then(() =>
                              invoke("start_audio_test", {
                                deviceName: val === "default" ? null : val,
                              })
                            ).catch(() => {});
                          }
                        }}
                        startAdornment={<MicIcon sx={{ fontSize: 18, mr: 1, color: "text.secondary" }} />}
                        sx={{ borderRadius: "12px" }}
                      >
                        {audioDevices.inputDevices.map((device) => (
                          <MenuItem key={device.name} value={device.name}>
                            {device.name} {device.isDefault && t("settings.default")}
                          </MenuItem>
                        ))}
                      </Select>
                    </FormControl>
                    <Button
                      size="small"
                      variant={micTesting ? "contained" : "outlined"}
                      color={micTesting ? "error" : "primary"}
                      onClick={handleToggleMicTest}
                      sx={{
                        minWidth: 64,
                        borderRadius: "10px",
                        height: 40,
                        textTransform: "none",
                        fontSize: "0.8rem",
                        flexShrink: 0,
                      }}
                    >
                      {micTesting ? t("common.stop") : t("common.test")}
                    </Button>
                  </Box>

                  {micTesting && (
                    <Box sx={{ mb: 2 }}>
                      <AudioLevelMeter compact />
                    </Box>
                  )}

                  <Box sx={{ display: "flex", gap: 1, alignItems: "flex-end" }}>
                    <FormControl fullWidth size="small">
                      <InputLabel>{t("settings.speaker")}</InputLabel>
                      <Select
                        value={selectedOutputDevice}
                        label={t("settings.speaker")}
                        onChange={(e) => {
                          const val = e.target.value;
                          setSelectedOutputDeviceLocal(val);
                          storeSetOutputDevice(val);
                          invoke("set_audio_devices", {
                            inputDevice: selectedInputDevice === "default" ? null : selectedInputDevice,
                            outputDevice: val === "default" ? null : val,
                          }).catch(() => {});
                        }}
                        startAdornment={<VolumeUpIcon sx={{ fontSize: 18, mr: 1, color: "text.secondary" }} />}
                        sx={{ borderRadius: "12px" }}
                      >
                        {audioDevices.outputDevices.map((device) => (
                          <MenuItem key={device.name} value={device.name}>
                            {device.name} {device.isDefault && t("settings.default")}
                          </MenuItem>
                        ))}
                      </Select>
                    </FormControl>
                    <Button
                      size="small"
                      variant="outlined"
                      onClick={handlePlayTestTone}
                      disabled={tonePlaying}
                      sx={{
                        minWidth: 64,
                        borderRadius: "10px",
                        height: 40,
                        textTransform: "none",
                        fontSize: "0.8rem",
                        flexShrink: 0,
                      }}
                    >
                      {tonePlaying ? "..." : t("common.test")}
                    </Button>
                  </Box>

                  <Typography variant="caption" sx={{ display: "block", mt: 1.5, color: "text.secondary" }}>
                    {t("settings.audioNote")}
                  </Typography>
                </>
              )}
              {!audioDevices && (
                <Typography variant="body2" sx={{ color: "text.secondary", textAlign: "center", py: 2 }}>
                  {t("settings.loadingAudio")}
                </Typography>
              )}
            </Box>
          </Collapse>

          {/* Contacts sync section */}
          <ListItemButton
            onClick={() => setShowContactsSettings(!showContactsSettings)}
            sx={{ borderRadius: "16px", mb: 0.5 }}
          >
            <ListItemIcon sx={{ minWidth: 40 }}>
              <ContactsIcon sx={{ fontSize: 20 }} />
            </ListItemIcon>
            <ListItemText
              primary={t("settings.contactsSync")}
              secondary={t("settings.syncedContacts", { count: systemContactCount + googleContactCount })}
              primaryTypographyProps={{ fontSize: "0.9rem" }}
              secondaryTypographyProps={{ fontSize: "0.7rem" }}
            />
            {showContactsSettings ? <ExpandLessIcon sx={{ fontSize: 20 }} /> : <ExpandMoreIcon sx={{ fontSize: 20 }} />}
          </ListItemButton>

          <Collapse in={showContactsSettings}>
            <Box sx={{ px: 2, pb: 2 }}>
              {/* System Contacts (macOS only for now) */}
              <Box
                sx={{
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                  py: 1,
                }}
              >
                <Box sx={{ display: "flex", alignItems: "center", gap: 1.5 }}>
                  <PhoneIphoneIcon sx={{ fontSize: 20, color: "text.secondary" }} />
                  <Box>
                    <Typography variant="body2">{t("settings.systemContacts")}</Typography>
                    <Typography variant="caption" sx={{ color: "text.secondary" }}>
                      {systemContactsEnabled
                        ? t("settings.contactsSyncStatus", { count: systemContactCount, date: formatLastSync(systemLastSync) })
                        : t("settings.importFromDevice")}
                    </Typography>
                  </Box>
                </Box>
                <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
                  {systemContactsEnabled && (
                    <IconButton
                      size="small"
                      onClick={handleSyncSystemContacts}
                      disabled={syncingSystemContacts}
                    >
                      {syncingSystemContacts ? (
                        <CircularProgress size={18} />
                      ) : (
                        <SyncIcon sx={{ fontSize: 18 }} />
                      )}
                    </IconButton>
                  )}
                  <Switch
                    checked={systemContactsEnabled}
                    onChange={(e) => handleToggleSystemContacts(e.target.checked)}
                    size="small"
                    disabled={syncingSystemContacts}
                  />
                </Box>
              </Box>

              {/* Google Contacts (placeholder - requires OAuth setup) */}
              <Box
                sx={{
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                  py: 1,
                  opacity: 0.5,
                }}
              >
                <Box sx={{ display: "flex", alignItems: "center", gap: 1.5 }}>
                  <CloudIcon sx={{ fontSize: 20, color: "text.secondary" }} />
                  <Box>
                    <Typography variant="body2">{t("settings.googleContacts")}</Typography>
                    <Typography variant="caption" sx={{ color: "text.secondary" }}>
                      {googleConnected
                        ? t("settings.contactsSyncStatus", { count: googleContactCount, date: formatLastSync(googleLastSync) })
                        : t("settings.googleComingSoon")}
                    </Typography>
                  </Box>
                </Box>
                <Switch
                  checked={googleConnected}
                  disabled
                  size="small"
                />
              </Box>

              <Typography variant="caption" sx={{ display: "block", mt: 1, color: "text.disabled" }}>
                {t("settings.contactsSyncNote")}
              </Typography>
            </Box>
          </Collapse>

          {/* Recordings */}
          <ListItemButton
            onClick={() => openRecordingsFolder().catch(() => {})}
            sx={{ borderRadius: "16px", mb: 0.5 }}
          >
            <ListItemIcon sx={{ minWidth: 40 }}>
              <FiberManualRecordIcon sx={{ fontSize: 20, color: "error.main" }} />
            </ListItemIcon>
            <ListItemText
              primary={t("settings.callRecordings")}
              secondary={recordingsDir ? t("settings.savedTo", { path: recordingsDir.split("/").slice(-2).join("/") }) : t("common.loading")}
              primaryTypographyProps={{ fontSize: "0.9rem" }}
              secondaryTypographyProps={{ fontSize: "0.7rem", noWrap: true }}
            />
            <FolderOpenIcon sx={{ fontSize: 20, color: "text.secondary" }} />
          </ListItemButton>
        </List>

        <Divider sx={{ my: 1.5, opacity: 0.15 }} />

        {/* Sign out all */}
        <List disablePadding>
          <ListItemButton
            onClick={handleSignOutAll}
            sx={{ borderRadius: "16px", mb: 0.5 }}
          >
            <ListItemIcon sx={{ minWidth: 40 }}>
              <LogoutIcon sx={{ fontSize: 20, color: "error.main" }} />
            </ListItemIcon>
            <ListItemText
              primary={t("settings.signOutAll")}
              primaryTypographyProps={{
                fontSize: "0.9rem",
                color: "error.main",
              }}
            />
          </ListItemButton>
        </List>

        {/* Developer tools */}
        <List disablePadding>
          <ListItemButton
            onClick={() => invoke("open_debug_window").catch(() => {})}
            sx={{ borderRadius: "16px", mb: 0.5 }}
          >
            <ListItemIcon sx={{ minWidth: 40 }}>
              <BugReportOutlinedIcon sx={{ fontSize: 20 }} />
            </ListItemIcon>
            <ListItemText
              primary={t("settings.developerDiagnostics")}
              secondary={t("settings.sipTraceRtp")}
              primaryTypographyProps={{ fontSize: "0.9rem" }}
              secondaryTypographyProps={{ fontSize: "0.7rem" }}
            />
          </ListItemButton>
        </List>

        <Divider sx={{ my: 1.5, opacity: 0.15 }} />

        {/* About */}
        <Box
          sx={{
            mx: 0.5,
            p: 2,
            borderRadius: "16px",
            display: "flex",
            alignItems: "flex-start",
            gap: 1.5,
          }}
        >
          <InfoOutlinedIcon
            sx={{ fontSize: 18, color: "text.secondary", mt: 0.2 }}
          />
          <Box>
            <Typography variant="body2" sx={{ fontWeight: 500, mb: 0.3 }}>
              {t("settings.about")}
            </Typography>
            <Typography variant="caption" sx={{ color: "text.secondary" }}>
              {t("settings.aboutDescription")}
              <br />
              {t("settings.aboutLicense")}
            </Typography>
          </Box>
        </Box>
      </Box>

      {/* Add/Edit Account Dialog */}
      <Dialog
        open={dialogOpen}
        onClose={handleCloseDialog}
        maxWidth="xs"
        fullWidth
        PaperProps={{
          sx: { borderRadius: "20px" },
        }}
      >
        <DialogTitle>
          {editingAccount ? t("settings.editAccount") : t("settings.addAccount")}
        </DialogTitle>
        <DialogContent>
          <Box
            sx={{ display: "flex", flexDirection: "column", gap: 2, pt: 1 }}
          >
            <TextField
              label={t("wizard.server")}
              size="small"
              value={form.server}
              onChange={(e) => update("server", e.target.value)}
              placeholder={t("wizard.serverPlaceholder")}
              autoFocus={!editingAccount}
              sx={inputSx}
            />
            <TextField
              label={t("wizard.username")}
              size="small"
              value={form.username}
              onChange={(e) => update("username", e.target.value)}
              placeholder={t("wizard.usernamePlaceholder")}
              sx={inputSx}
            />
            <TextField
              label={t("wizard.password")}
              size="small"
              type={showPassword ? "text" : "password"}
              value={form.password}
              onChange={(e) => update("password", e.target.value)}
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

            <Button
              size="small"
              onClick={() => setShowAdvancedInDialog(!showAdvancedInDialog)}
              endIcon={
                showAdvancedInDialog ? <ExpandLessIcon /> : <ExpandMoreIcon />
              }
              sx={{
                alignSelf: "flex-start",
                color: "text.secondary",
                textTransform: "none",
                fontSize: "0.8rem",
              }}
            >
              {t("wizard.advancedOptions")}
            </Button>

            <Collapse in={showAdvancedInDialog}>
              <Box
                sx={{ display: "flex", flexDirection: "column", gap: 2 }}
              >
                <TextField
                  label={t("wizard.displayName")}
                  size="small"
                  value={form.displayName}
                  onChange={(e) => update("displayName", e.target.value)}
                  placeholder={t("wizard.displayNamePlaceholder")}
                  sx={inputSx}
                />
                <TextField
                  label={t("wizard.authUsername")}
                  size="small"
                  value={form.authUsername}
                  onChange={(e) => update("authUsername", e.target.value)}
                  placeholder={t("wizard.authUsernamePlaceholder")}
                  sx={inputSx}
                />
                <TextField
                  label={t("wizard.authRealm")}
                  size="small"
                  value={form.authRealm}
                  onChange={(e) => update("authRealm", e.target.value)}
                  placeholder={t("wizard.authRealmPlaceholder")}
                  helperText={t("wizard.authRealmHelper")}
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
                  sx={inputSx}
                />
                <TextField
                  label={t("wizard.outboundProxy")}
                  size="small"
                  value={form.outboundProxy}
                  onChange={(e) => update("outboundProxy", e.target.value)}
                  placeholder={t("wizard.outboundProxyPlaceholder")}
                  sx={inputSx}
                />
                <Box sx={{ display: "flex", alignItems: "center", justifyContent: "space-between", mt: 1 }}>
                  <Typography variant="body2" sx={{ color: "text.secondary" }}>
                    {t("settings.autoRecord")}
                  </Typography>
                  <Switch
                    checked={form.autoRecord}
                    onChange={(e) => setForm(f => ({ ...f, autoRecord: e.target.checked }))}
                    size="small"
                  />
                </Box>
                <Typography variant="caption" sx={{ color: "text.disabled", mt: -0.5 }}>
                  {t("settings.autoRecordNote")}
                </Typography>

                {/* SRTP Mode */}
                <Box sx={{ mt: 2 }}>
                  <Typography variant="body2" sx={{ color: "text.secondary", mb: 1 }}>
                    {t("settings.srtpMode")}
                  </Typography>
                  <Box sx={{ display: "flex", gap: 1 }}>
                    {(["disabled", "sdes", "dtls"] as const).map((mode) => (
                      <Chip
                        key={mode}
                        label={t(`settings.srtp.${mode}`)}
                        size="small"
                        onClick={() => setForm(f => ({ ...f, srtpMode: mode }))}
                        color={form.srtpMode === mode ? "primary" : "default"}
                        variant={form.srtpMode === mode ? "filled" : "outlined"}
                        sx={{
                          borderRadius: "8px",
                          cursor: "pointer",
                        }}
                      />
                    ))}
                  </Box>
                  <Typography variant="caption" sx={{ color: "text.disabled", mt: 0.5, display: "block" }}>
                    {t("settings.srtpModeNote")}
                  </Typography>
                </Box>

                {/* Codec Priority Section */}
                <Divider sx={{ my: 1.5 }} />
                <Typography variant="subtitle2" sx={{ fontWeight: 600, mb: 1 }}>
                  {t("settings.codecPriority")}
                </Typography>
                <Typography variant="caption" sx={{ color: "text.disabled", mb: 1.5, display: "block" }}>
                  {t("settings.codecPriorityNote")}
                </Typography>
                <Box sx={{ display: "flex", flexDirection: "column", gap: 0.5 }}>
                  {form.codecs
                    .slice()
                    .sort((a, b) => a.priority - b.priority)
                    .map((codecConfig, index) => {
                      const info = CODEC_INFO[codecConfig.codec];
                      return (
                        <Box
                          key={codecConfig.codec}
                          sx={{
                            display: "flex",
                            alignItems: "center",
                            gap: 1,
                            p: 1,
                            borderRadius: "8px",
                            bgcolor: codecConfig.enabled
                              ? alpha(theme.palette.primary.main, 0.08)
                              : alpha(theme.palette.grey[500], 0.08),
                            border: "1px solid",
                            borderColor: codecConfig.enabled
                              ? alpha(theme.palette.primary.main, 0.2)
                              : "transparent",
                            opacity: codecConfig.enabled ? 1 : 0.6,
                          }}
                        >
                          <Box sx={{ display: "flex", flexDirection: "column", mr: 0.5 }}>
                            <IconButton
                              size="small"
                              disabled={index === 0}
                              onClick={() => {
                                setForm((f) => {
                                  const newCodecs = [...f.codecs];
                                  const thisIdx = newCodecs.findIndex(c => c.codec === codecConfig.codec);
                                  const prevIdx = newCodecs.findIndex(c => c.priority === codecConfig.priority - 1);
                                  if (thisIdx >= 0 && prevIdx >= 0) {
                                    const thisPriority = newCodecs[thisIdx].priority;
                                    newCodecs[thisIdx] = { ...newCodecs[thisIdx], priority: newCodecs[prevIdx].priority };
                                    newCodecs[prevIdx] = { ...newCodecs[prevIdx], priority: thisPriority };
                                  }
                                  return { ...f, codecs: newCodecs };
                                });
                              }}
                              sx={{ p: 0.25 }}
                            >
                              <ArrowUpwardIcon sx={{ fontSize: 14 }} />
                            </IconButton>
                            <IconButton
                              size="small"
                              disabled={index === form.codecs.length - 1}
                              onClick={() => {
                                setForm((f) => {
                                  const newCodecs = [...f.codecs];
                                  const thisIdx = newCodecs.findIndex(c => c.codec === codecConfig.codec);
                                  const nextIdx = newCodecs.findIndex(c => c.priority === codecConfig.priority + 1);
                                  if (thisIdx >= 0 && nextIdx >= 0) {
                                    const thisPriority = newCodecs[thisIdx].priority;
                                    newCodecs[thisIdx] = { ...newCodecs[thisIdx], priority: newCodecs[nextIdx].priority };
                                    newCodecs[nextIdx] = { ...newCodecs[nextIdx], priority: thisPriority };
                                  }
                                  return { ...f, codecs: newCodecs };
                                });
                              }}
                              sx={{ p: 0.25 }}
                            >
                              <ArrowDownwardIcon sx={{ fontSize: 14 }} />
                            </IconButton>
                          </Box>
                          <Box sx={{ flex: 1 }}>
                            <Typography variant="body2" sx={{ fontWeight: 500 }}>
                              {info.name}
                            </Typography>
                            <Typography variant="caption" sx={{ color: "text.secondary" }}>
                              {info.bitrate} • {info.description}
                            </Typography>
                          </Box>
                          <Switch
                            size="small"
                            checked={codecConfig.enabled}
                            onChange={(e) => {
                              setForm((f) => ({
                                ...f,
                                codecs: f.codecs.map((c) =>
                                  c.codec === codecConfig.codec
                                    ? { ...c, enabled: e.target.checked }
                                    : c
                                ),
                              }));
                            }}
                          />
                        </Box>
                      );
                    })}
                </Box>
              </Box>
            </Collapse>
          </Box>
        </DialogContent>
        <DialogActions sx={{ px: 3, pb: 2 }}>
          <Button onClick={handleCloseDialog} sx={{ borderRadius: "12px" }}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="contained"
            onClick={handleSaveAccount}
            disabled={!canSave || saving}
            sx={{ borderRadius: "12px" }}
          >
            {saving ? (
              <CircularProgress size={18} color="inherit" />
            ) : editingAccount ? (
              t("common.save")
            ) : (
              t("common.add")
            )}
          </Button>
        </DialogActions>
      </Dialog>
    </Box>
  );
}
