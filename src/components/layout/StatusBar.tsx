import { useState, useMemo } from "react";
import { Box, Chip, alpha, useTheme, Menu, MenuItem, Typography } from "@mui/material";
import CircleIcon from "@mui/icons-material/Circle";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import CheckIcon from "@mui/icons-material/Check";
import AccountCircleIcon from "@mui/icons-material/AccountCircle";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { sipSetActiveAccount } from "../../hooks/useSip";

const stateColors: Record<string, string> = {
  registered: "#4caf50",
  registering: "#ff9800",
  reconnecting: "#ff9800",
  unregistered: "#9e9e9e",
  error: "#f44336",
};

const stateLabelsKeys: Record<string, string> = {
  registered: "status.connected",
  registering: "status.connecting",
  reconnecting: "status.reconnecting",
  unregistered: "status.notConnected",
  error: "status.connectionError",
};

export function StatusBar() {
  const { t } = useTranslation();
  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const setActiveAccount = useAppStore((s) => s.setActiveAccount);
  const accountStates = useAppStore((s) => s.accountStates);
  const theme = useTheme();

  const [menuAnchor, setMenuAnchor] = useState<null | HTMLElement>(null);

  const activeAccount = accounts.find((a) => a.id === activeAccountId);
  const activeState = activeAccountId ? accountStates[activeAccountId] : null;
  const registrationState = activeState?.registrationState ?? "unregistered";

  const availableAccounts = useMemo(() => {
    return accounts.filter((a) => {
      if (!a.enabled) return false;
      const state = accountStates[a.id]?.registrationState;
      return state === "registered" || state === "registering" || state === "reconnecting";
    });
  }, [accounts, accountStates]);

  const hasMultipleAccounts = availableAccounts.length > 1;

  const handleAccountSelect = async (accountId: string) => {
    setMenuAnchor(null);
    if (accountId === activeAccountId) return;
    setActiveAccount(accountId);
    await sipSetActiveAccount(accountId).catch(() => {});
  };

  const color = stateColors[registrationState];

  return (
    <Box
      sx={{
        px: 2,
        py: 1,
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        borderBottom: `1px solid ${alpha(theme.palette.divider, 0.12)}`,
        minHeight: 44,
        WebkitAppRegion: "drag",
        userSelect: "none",
      }}
    >
      {hasMultipleAccounts ? (
        <>
          <Box
            onClick={(e) => setMenuAnchor(e.currentTarget)}
            sx={{
              display: "flex",
              alignItems: "center",
              gap: 0.5,
              fontSize: "0.8rem",
              color: "text.secondary",
              cursor: "pointer",
              px: 1,
              py: 0.5,
              mx: -1,
              borderRadius: "8px",
              WebkitAppRegion: "no-drag",
              "&:hover": {
                bgcolor: alpha(theme.palette.primary.main, 0.08),
              },
            }}
          >
            {activeAccount?.displayName || t("status.noAccount")}
            <ExpandMoreIcon sx={{ fontSize: 16, opacity: 0.6 }} />
          </Box>
          <Menu
            anchorEl={menuAnchor}
            open={Boolean(menuAnchor)}
            onClose={() => setMenuAnchor(null)}
            anchorOrigin={{ vertical: "bottom", horizontal: "left" }}
            transformOrigin={{ vertical: "top", horizontal: "left" }}
            slotProps={{
              paper: {
                sx: {
                  borderRadius: "12px",
                  minWidth: 200,
                  mt: 0.5,
                },
              },
            }}
          >
            {availableAccounts.map((account) => {
              const isActive = account.id === activeAccountId;
              const state = accountStates[account.id]?.registrationState;
              const stateColor = stateColors[state || "unregistered"];
              return (
                <MenuItem
                  key={account.id}
                  onClick={() => handleAccountSelect(account.id)}
                  selected={isActive}
                  sx={{
                    borderRadius: "8px",
                    mx: 0.5,
                    px: 1.5,
                    py: 1,
                    display: "flex",
                    alignItems: "center",
                    gap: 1,
                  }}
                >
                  <AccountCircleIcon sx={{ fontSize: 20, color: "text.secondary" }} />
                  <Box sx={{ flex: 1 }}>
                    <Typography variant="body2" sx={{ fontWeight: isActive ? 600 : 400 }}>
                      {account.displayName || account.username}
                    </Typography>
                    <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
                      <CircleIcon sx={{ fontSize: 6, color: stateColor }} />
                      <Typography variant="caption" sx={{ color: "text.secondary" }}>
                        {account.username}@{account.domain}
                      </Typography>
                    </Box>
                  </Box>
                  {isActive && <CheckIcon sx={{ fontSize: 18, color: "primary.main" }} />}
                </MenuItem>
              );
            })}
          </Menu>
        </>
      ) : (
        <Box sx={{ fontSize: "0.8rem", color: "text.secondary" }}>
          {activeAccount ? activeAccount.displayName : t("status.noAccount")}
        </Box>
      )}
      <Chip
        icon={<CircleIcon sx={{ fontSize: 8, color: `${color} !important` }} />}
        label={t(stateLabelsKeys[registrationState])}
        size="small"
        variant="outlined"
        sx={{
          height: 24,
          fontSize: "0.7rem",
          borderColor: alpha(color, 0.3),
          color: "text.secondary",
          WebkitAppRegion: "no-drag",
        }}
      />
    </Box>
  );
}
