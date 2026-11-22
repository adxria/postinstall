#[derive(Clone, Copy)]
pub enum Target {
    Settings(&'static str),
    SystemProgram(&'static str, &'static str),
    DarkTheme,
    Apply(Preference),
}

#[derive(Clone, Copy)]
pub enum Preference {
    ClassicContextMenu,
    PrintScreen,
    MouseAcceleration,
    KeyboardRepeat,
    StickyKeysShortcut,
    AccessKeyUnderlines,
}

pub struct Recommendation {
    pub title: &'static str,
    pub advice: &'static str,
    pub path: &'static str,
    pub target: Target,
    pub apply: Option<Preference>,
}

use Preference::*;
use Target::{Apply, DarkTheme, Settings, SystemProgram};

pub static RECOMMENDATIONS: &[Recommendation] = &[
    Recommendation {
        title: "Windows dark theme",
        advice: "Apply Windows’ built-in dark theme, including its wallpaper. Use the theme file if Personalization is locked pending activation.",
        path: r"%WINDIR%\Resources\Themes\dark.theme",
        target: DarkTheme,
        apply: None,
    },
    Recommendation {
        title: "Animations and visual effects",
        advice: "Choose Custom and turn on these four options.\r\n\r\n• Enable Peek\r\n• Show thumbnails instead of icons\r\n• Show window contents while dragging\r\n• Smooth edges of screen fonts",
        path: "Performance Options → Visual Effects",
        target: SystemProgram("SystemPropertiesPerformance.exe", ""),
        apply: None,
    },
    Recommendation {
        title: "File Explorer options",
        advice: "General → Set Open File Explorer to This PC.\r\n\r\nView → Select Show hidden files, folders, and drives. Clear Hide extensions for known file types.",
        path: "File Explorer → See more (⋯) → Options",
        target: SystemProgram("control.exe", "folders"),
        apply: None,
    },
    Recommendation {
        title: "Classic context menu",
        advice: "Use the classic right-click menu by default. This Explorer tweak can break after a Windows update.",
        path: "File Explorer → Right-click a file or folder → Show more options",
        target: Apply(ClassicContextMenu),
        apply: None,
    },
    Recommendation {
        title: "Print Screen behavior",
        advice: "Make Print Screen copy the whole screen to the clipboard. Use Win+Shift+S for snipping.",
        path: "Sign out and back in after applying.",
        target: Apply(PrintScreen),
        apply: None,
    },
    Recommendation {
        title: "Mouse pointer options",
        advice: "Clear Enhance pointer precision to turn off mouse acceleration.",
        path: "Control Panel → Mouse → Pointer Options",
        target: SystemProgram("control.exe", "main.cpl,,2"),
        apply: Some(MouseAcceleration),
    },
    Recommendation {
        title: "Keyboard repeat",
        advice: "Use a short repeat delay and a fast repeat rate. Hold a key in the test box to check the result.",
        path: "Control Panel → Keyboard → Speed",
        target: SystemProgram("control.exe", "keyboard"),
        apply: Some(KeyboardRepeat),
    },
    Recommendation {
        title: "Sticky Keys shortcut",
        advice: "Clear Turn on Sticky Keys when SHIFT is pressed five times to prevent accidental activation.",
        path: "Control Panel → Ease of Access Center → Make the keyboard easier to use → Set up Sticky Keys",
        target: SystemProgram(
            "control.exe",
            "/name Microsoft.EaseOfAccessCenter /page pageStickyKeysSettings",
        ),
        apply: Some(StickyKeysShortcut),
    },
    Recommendation {
        title: "Underline access keys",
        advice: "Turn on Underline access keys so supported apps show which letters activate menu items.",
        path: "Settings → Accessibility → Keyboard → Underline access keys",
        target: Settings("ms-settings:easeofaccess-keyboard"),
        apply: Some(AccessKeyUnderlines),
    },
    Recommendation {
        title: "Language and keyboard layouts",
        advice: "Keep only the languages and keyboard layouts you use. Put your preferred language first.",
        path: "Settings → Time & language → Language & region",
        target: Settings("ms-settings:regionlanguage"),
        apply: None,
    },
    Recommendation {
        title: "Input language shortcuts",
        advice: "Use Win+Space to switch layouts. Disable any legacy switching shortcuts you hit by accident.",
        path: "Settings → Time & language → Typing → Advanced keyboard settings → Input language hot keys",
        target: SystemProgram(
            "rundll32.exe",
            "Shell32.dll,Control_RunDLL input.dll,,{C07337D3-DB2C-4D0B-9A93-B722A6C106E2}{HOTKEYS}",
        ),
        apply: None,
    },
    Recommendation {
        title: "Date and time",
        advice: "Turn on Set time automatically and check the time zone. Set the zone manually if Windows gets it wrong.",
        path: "Settings → Time & language → Date & time",
        target: Settings("ms-settings:dateandtime"),
        apply: None,
    },
    Recommendation {
        title: "Computer name",
        advice: "Give the PC a short, recognizable name. Restart after renaming.",
        path: "Settings → System → About → Rename this PC",
        target: SystemProgram("SystemPropertiesComputerName.exe", ""),
        apply: None,
    },
    Recommendation {
        title: "Screen and sleep timeouts",
        advice: "Set the screen to turn off before the PC sleeps. Use shorter delays on battery.",
        path: "Control Panel → Power Options → Change plan settings",
        target: SystemProgram(
            "control.exe",
            "/name Microsoft.PowerOptions /page pagePlanSettings",
        ),
        apply: None,
    },
    Recommendation {
        title: "Fast Startup",
        advice: "Turn off Fast Startup so Shut down performs a full shutdown. The next boot may take longer.",
        path: "Control Panel → Power Options → Choose what the power buttons do → Change settings that are currently unavailable",
        target: SystemProgram(
            "control.exe",
            "/name Microsoft.PowerOptions /page pageGlobalSettings",
        ),
        apply: None,
    },
];
