import { createTheme, type ThemeOptions } from "@mui/material/styles";

const shared: ThemeOptions = {
  typography: {
    fontFamily: '"Google Sans", "Google Sans Text", "Roboto", sans-serif',
    h4: { fontWeight: 500, letterSpacing: "-0.02em" },
    h5: { fontWeight: 500, letterSpacing: "-0.01em" },
    h6: { fontWeight: 500 },
    button: { fontWeight: 500, textTransform: "none" },
  },
  shape: { borderRadius: 16 },
  components: {
    MuiButton: {
      styleOverrides: {
        root: {
          borderRadius: 20,
          padding: "10px 24px",
          fontSize: "0.95rem",
        },
      },
    },
    MuiCard: {
      styleOverrides: {
        root: {
          borderRadius: 20,
          backgroundImage: "none",
        },
      },
    },
    MuiPaper: {
      styleOverrides: {
        root: {
          backgroundImage: "none",
        },
      },
    },
    MuiFab: {
      styleOverrides: {
        root: {
          borderRadius: 16,
        },
      },
    },
    MuiListItemButton: {
      styleOverrides: {
        root: {
          borderRadius: 28,
          "&.Mui-selected": {
            fontWeight: 500,
          },
        },
      },
    },
    MuiIconButton: {
      styleOverrides: {
        root: {
          borderRadius: 12,
        },
      },
    },
  },
};

export const lightTheme = createTheme({
  ...shared,
  palette: {
    mode: "light",
    primary: {
      main: "#1a6b52",
      light: "#4e9d84",
      dark: "#004d38",
      contrastText: "#ffffff",
    },
    secondary: {
      main: "#4f6354",
      light: "#7d9182",
      dark: "#233829",
      contrastText: "#ffffff",
    },
    error: {
      main: "#ba1a1a",
    },
    background: {
      default: "#f5fbf5",
      paper: "#edf3ed",
    },
    text: {
      primary: "#171d19",
      secondary: "#404943",
    },
  },
});

export const darkTheme = createTheme({
  ...shared,
  palette: {
    mode: "dark",
    primary: {
      main: "#7ddcb0",
      light: "#a8f0cf",
      dark: "#005138",
      contrastText: "#003826",
    },
    secondary: {
      main: "#b3ccb8",
      light: "#cfe8d4",
      dark: "#3b4f40",
      contrastText: "#223528",
    },
    error: {
      main: "#ffb4ab",
    },
    background: {
      default: "#0f1511",
      paper: "#1b211d",
    },
    divider: "rgba(255,255,255,0.10)",
    text: {
      primary: "#e8ede8",
      secondary: "#c8d2c9",
    },
  },
});
