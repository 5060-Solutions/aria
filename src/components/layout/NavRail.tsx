import { Badge, Box, ButtonBase, Tooltip, alpha, useTheme } from "@mui/material";
import DialpadIcon from "@mui/icons-material/Dialpad";
import HistoryIcon from "@mui/icons-material/History";
import ContactsIcon from "@mui/icons-material/People";
import VoicemailIcon from "@mui/icons-material/Voicemail";
import ChatIcon from "@mui/icons-material/ChatBubbleOutlined";
import GraphicEqIcon from "@mui/icons-material/GraphicEq";
import SettingsIcon from "@mui/icons-material/Settings";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { useMessagingStore } from "../../stores/messagingStore";
import { mailboxFor } from "../../utils/voicemail";
import { showRecordingsNav } from "../../utils/recordings";

function AriaLogo({ size = 40 }: { size?: number }) {
  return (
    <svg viewBox="0 0 512 512" width={size} height={size}>
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
  );
}

const navItems = [
  { id: "dialer" as const, icon: DialpadIcon, labelKey: "nav.dialer" },
  { id: "history" as const, icon: HistoryIcon, labelKey: "nav.history" },
  { id: "contacts" as const, icon: ContactsIcon, labelKey: "nav.contacts" },
  // Only for an account on an Aria PBX (it has a PBX API address); anyone
  // else reaches voicemail by dialling, from the status bar button.
  { id: "voicemail" as const, icon: VoicemailIcon, labelKey: "nav.voicemail" },
  // Only when the PBX lists a messaging number the extension may use.
  { id: "messages" as const, icon: ChatIcon, labelKey: "nav.messages" },
  // Once a call has been recorded, or an account records every call: where
  // the recordings and their transcripts live.
  { id: "recordings" as const, icon: GraphicEqIcon, labelKey: "nav.recordings" },
  { id: "settings" as const, icon: SettingsIcon, labelKey: "nav.settings" },
];

export function NavRail() {
  const { t } = useTranslation();
  const currentView = useAppStore((s) => s.currentView);
  const setCurrentView = useAppStore((s) => s.setCurrentView);
  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const voicemail = useAppStore((s) => s.voicemail);
  const callHistory = useAppStore((s) => s.callHistory);
  const theme = useTheme();
  const hasMailbox = mailboxFor(accounts.find((a) => a.id === activeAccountId)) !== null;
  const hasRecordings = showRecordingsNav(callHistory, accounts);
  const inboxes = useMessagingStore((s) => (activeAccountId ? s.byAccount[activeAccountId]?.inboxes : undefined));
  const conversations = useMessagingStore((s) => s.conversations);
  const seen = useMessagingStore((s) => s.seen);
  const unseen = useMessagingStore((s) => s.unseen);
  const hasMessaging = (inboxes?.length ?? 0) > 0;
  // Re-derived whenever a list or the seen record changes.
  void conversations;
  void seen;
  const unseenMessages = (inboxes ?? []).reduce((n, i) => n + unseen(i.didId), 0);
  const newVoicemail = activeAccountId ? voicemail[activeAccountId]?.newMessages ?? 0 : 0;
  const items = navItems.filter(
    (item) =>
      (item.id !== "voicemail" || hasMailbox) &&
      (item.id !== "messages" || hasMessaging) &&
      (item.id !== "recordings" || hasRecordings),
  );

  return (
    <Box
      sx={{
        width: 72,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        py: 2,
        gap: 0.5,
        bgcolor: "background.paper",
        borderRight: `1px solid ${alpha(theme.palette.divider, 0.12)}`,
      }}
    >
      <Box sx={{ mb: 1.5 }}>
        <AriaLogo size={36} />
      </Box>

      {items.map(({ id, icon: Icon, labelKey }) => {
        const active = currentView === id;
        const label = t(labelKey);
        return (
          <Tooltip key={id} title={label} placement="right">
            {/* One button for the icon and the word under it: the label used
                to be plain text, so clicking it did nothing. */}
            <ButtonBase
              onClick={() => setCurrentView(id)}
              aria-label={label}
              aria-current={active ? "page" : undefined}
              focusRipple
              sx={{
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                gap: 0.5,
                width: "100%",
                px: 1.5,
                py: 0.5,
                borderRadius: "12px",
                color: active ? "primary.main" : "text.secondary",
                "&:hover .nav-pill": {
                  bgcolor: active
                    ? alpha(theme.palette.primary.main, 0.24)
                    : alpha(theme.palette.text.secondary, 0.08),
                },
              }}
            >
              <Box
                className="nav-pill"
                sx={{
                  width: 48,
                  height: 32,
                  borderRadius: "16px",
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  bgcolor: active
                    ? alpha(theme.palette.primary.main, 0.16)
                    : "transparent",
                  transition: "all 0.2s ease",
                }}
              >
                <Badge
                  badgeContent={id === "voicemail" ? newVoicemail : id === "messages" ? unseenMessages : 0}
                  color="error"
                  max={99}
                  overlap="circular"
                >
                  <Icon fontSize="small" />
                </Badge>
              </Box>
              <Box
                sx={{
                  fontSize: "0.7rem",
                  fontWeight: active ? 600 : 400,
                  lineHeight: 1,
                }}
              >
                {label}
              </Box>
            </ButtonBase>
          </Tooltip>
        );
      })}
    </Box>
  );
}
