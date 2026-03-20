import { Box, IconButton, Tooltip, alpha, useTheme } from "@mui/material";
import DialpadIcon from "@mui/icons-material/Dialpad";
import HistoryIcon from "@mui/icons-material/History";
import ContactsIcon from "@mui/icons-material/People";
import SettingsIcon from "@mui/icons-material/Settings";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";

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
  { id: "settings" as const, icon: SettingsIcon, labelKey: "nav.settings" },
];

export function NavRail() {
  const { t } = useTranslation();
  const currentView = useAppStore((s) => s.currentView);
  const setCurrentView = useAppStore((s) => s.setCurrentView);
  const theme = useTheme();

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
      <Box sx={{ mb: 2 }}>
        <AriaLogo size={40} />
      </Box>

      {navItems.map(({ id, icon: Icon, labelKey }) => {
        const active = currentView === id;
        const label = t(labelKey);
        return (
          <Tooltip key={id} title={label} placement="right">
            <Box
              sx={{
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                gap: 0.5,
                width: "100%",
                px: 1.5,
              }}
            >
              <IconButton
                onClick={() => setCurrentView(id)}
                sx={{
                  width: 48,
                  height: 32,
                  borderRadius: "16px",
                  bgcolor: active
                    ? alpha(theme.palette.primary.main, 0.16)
                    : "transparent",
                  color: active ? "primary.main" : "text.secondary",
                  transition: "all 0.2s ease",
                  "&:hover": {
                    bgcolor: active
                      ? alpha(theme.palette.primary.main, 0.24)
                      : alpha(theme.palette.text.secondary, 0.08),
                  },
                }}
              >
                <Icon fontSize="small" />
              </IconButton>
              <Box
                sx={{
                  fontSize: "0.7rem",
                  fontWeight: active ? 600 : 400,
                  color: active ? "primary.main" : "text.secondary",
                  lineHeight: 1,
                }}
              >
                {label}
              </Box>
            </Box>
          </Tooltip>
        );
      })}
    </Box>
  );
}
