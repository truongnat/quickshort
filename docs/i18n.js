// Quickshort landing — EN/VI strings. English is the default; `?lang=vi`,
// then the saved choice, override it. Elements bind via data-i18n (innerHTML)
// and data-i18n-ph (placeholder); JS pulls the rest through QS_T().
window.QS_I18N = {
  en: {
    "meta.title": "Quickshort - Region screenshots at native speed",
    "meta.desc": "Multi-monitor region screenshots with annotations and native OCR. Pure Rust, no Electron.",

    "nav.sponsor": "Sponsor",
    "nav.download": "Download",

    "hero.chip": "NATIVE SCREENSHOT TOOL",
    "hero.w1": "Select.",
    "hero.w2": "Snip.",
    "hero.w3": "Ship.",
    "hero.sub": "Pixel-perfect region captures across every monitor, with annotations and native OCR. Pure Rust, no Electron.",
    "hero.gh": "View on GitHub",

    "strip.1": "RETINA-NATIVE",
    "strip.2": "MULTI-MONITOR",
    "strip.3": "ANNOTATIONS",
    "strip.4": "NATIVE OCR",
    "strip.5": "ZERO ELECTRON",
    "strip.6": "PURE RUST",

    "sh.eyebrow": "SHOWCASE",
    "sh.h2": "The whole loop, on screen.",
    "sh.t1": "DEMO",
    "sh.t2": "OVERLAY",
    "sh.t3": "ANNOTATE",
    "sh.t4": "TRAY",
    "sh.t5": "SETTINGS",

    "feat.h2": "Everything a snip should do.<br>Nothing it should not.",
    "f1.h": "Annotate in place",
    "f1.p": "Draw directly on the dimmed overlay — pen, arrow, rectangle, ellipse, highlighter, text, pixelate — everything confined to the captured region. The text tool is a real editing box: font sizes (12–48), bold toggle, caret, selection and ⌘A, with IME input for Vietnamese. Every shape and text block stays editable — the Select tool picks it up, drags it somewhere better, or deletes it. Strokes live on a physical-resolution layer, so they stay as sharp as the capture itself. Undo and redo are one click.",
    "f2.h": "Global hotkey, resident tray",
    "f2.p": "Quickshort sits in the menu bar. Press ⌘⇧S from anywhere and a dimmed overlay covers every monitor at once. Esc cancels, and the shortcut is re-recordable in Settings.",
    "f3.h": "Physical pixels, always",
    "f3.p": "Captures request framebuffer resolution, not logical points. On a Retina display the PNG is 2880px wide and text stays crisp at 100% zoom. Mixed-DPI setups map each monitor through its own scale.",
    "f4.h": "OCR with OS engines",
    "f4.p": "⌘T runs the selection through macOS Vision (Windows.Media.Ocr on Windows) and puts the text on your clipboard with a toast confirmation. Native engines, no bundled models, no cloud calls.",
    "f5.h": "Cross-monitor selection",
    "f5.p": "Start a drag on one display and finish it on another. Each monitor's crop is mapped through its own origin and DPI, then stitched into a single image.",
    "f6.h": "Share links that outlive the app",
    "f6.p": "⌘L uploads the capture to a Cloudflare edge worker and drops a public link on your clipboard — still serving the image 7 days later, long after you quit.",
    "f7.h": "Local-first, no account",
    "f7.p": "No sign-up, no telemetry, nothing leaves your machine. Captures land in your folder, settings live in a plain config file, and sharing runs through a minimal relay — not third-party storage.",
    "f8.h": "Scriptable one-shot",
    "f8.p": "<code>quickshort --capture --out shot.png</code> skips the tray lifecycle — select, act, exit. <code>--delay</code> waits for menus and tooltips, so it composes with launchd, CI jobs, or a shell alias.",
    "f9.h": "Self-updating",
    "f9.p": "The tray checks GitHub Releases once a day. An update downloads the new .app, verifies its SHA-256, swaps it in place with rollback on failure, and relaunches. A stable self-signed identity keeps Screen Recording and Accessibility grants intact.",
    "f10.h": "Screen recording",
    "f10.p": "⌘R turns the selection into a live mp4 — PipeWire on Wayland, ScreenCaptureKit on macOS, GDI on Windows. ffmpeg encodes on a worker at 15fps with wall-clock duration, so a slow encode drops frames instead of shrinking the clip. A floating REC card ends it.",
    "f11.h": "Scrolling capture",
    "f11.p": "⌘G re-grabs the region while you scroll and stitches the frames into one tall image — long docs, threads, and chat logs that never fit on one screen.",
    "f12.h": "Pin & beautify",
    "f12.p": "Pin floats a capture always-on-top as a reference you can drag anywhere. Beautify mounts it on a soft gradient backdrop with rounded corners and a drop shadow — ready for docs and posts.",

    "pipe.eyebrow": "THE PIPELINE",
    "pipe.h2": "Four layers. Zero browsers.",
    "p1.h": "Capture",
    "p1.p": "Every monitor lands in RAM in one shot. ScreenCaptureKit, GDI BitBlt, or PipeWire shm.",
    "p2.h": "Overlay",
    "p2.p": "Borderless window per monitor dims the frames and tracks the drag in virtual coordinates.",
    "p3.h": "Map",
    "p3.p": "Selection coordinates resolve through each monitor's origin and DPI scale factor.",
    "p4.h": "Stitch",
    "p4.p": "Per-monitor crops merge on a worker thread while the event loop stays free.",

    "dl.h2": "Grab it.",
    "dl.mac": "Detected macOS. DMG or Homebrew.",
    "dl.win": "Detected Windows. One setup exe.",
    "dl.linux": "Detected Linux. AppImage or .deb.",
    "dl.cta": "Download for {os}",
    "dl.m1.d": "Download, open, drag to Applications. Unsigned build: right-click, Open, once.",
    "dl.m1.b": "Latest release",
    "dl.m2.d": "Custom tap, quarantine stripped for you.",
    "dl.w1.d": "Setup wizard: installs to Program Files, Start Menu shortcut, uninstaller included. Unsigned build: SmartScreen will warn once.",
    "dl.w1.b": "Download .exe",
    "dl.w2.d": "Prefer to build it yourself? GDI capture and Windows.Media.Ocr backends are native.",
    "dl.l1.d": "Portable, no install. Download, chmod +x, run. Bundles the tray dependencies.",
    "dl.l1.b": "Download AppImage",
    "dl.l2.d": "Registers a .desktop entry and icon, lands in /usr/bin.",
    "dl.perm": "macOS 14+ needs Screen Recording and Accessibility permissions. Windows installer is unsigned (SmartScreen once). Linux OCR runs Tesseract with script auto-detection. Screen recording (⌘R) needs ffmpeg on PATH — brew, winget, or apt.",

    "req.eyebrow": "FEEDBACK",
    "req.h2": "Shape what ships next.",
    "req.desc": "Bug reports get filed straight from the app — ideas come from here. Tell us what Quickshort should learn next. No account needed; it lands as a GitHub issue under the hood.",
    "req.title": "Short title — e.g. “Pin a capture on screen”",
    "req.note": "What should it do? How would it help your workflow?",
    "req.contact": "Contact (optional — GitHub @handle or email)",
    "req.send": "Send request",
    "req.sending": "Sending…",
    "req.sent": "Sent — track it on GitHub",
    "req.fail": "Could not send — try GitHub issues",

    "donate.eyebrow": "SUPPORT",
    "donate.h2": "Free for everyone.<br>Funded by a few.",
    "donate.gh": "International — card or PayPal. Personal sponsorships carry zero fees.",
    "donate.gh.b": "Become a sponsor",
    "donate.qr": "Domestic — scan with any banking app, lands straight in the account.",

    "sup.title": "THANKS TO",
    "sup.empty": "Be the first name on this wall.",
    "sup.name": "Your name (shown publicly)",
    "sup.send": "Get on the wall",
    "sup.sending": "Sending…",
    "sup.sent": "Got it — see you on the wall",
    "sup.fail": "Could not send",
    "sup.hint": "Donated via VietQR? Drop your name — it appears here once the transfer lands.",

    "ui.copy": "Copy",
    "ui.copied": "Copied",
    "ui.failed": "Failed",
    "os.macos": "macOS",
    "os.windows": "Windows",
    "os.linux": "Linux",
  },

  vi: {
    "meta.title": "Quickshort - Chụp vùng màn hình tốc độ native",
    "meta.desc": "Chụp vùng màn hình đa màn hình kèm chú thích và OCR native. Thuần Rust, không Electron.",

    "nav.sponsor": "Ủng hộ",
    "nav.download": "Tải về",

    "hero.chip": "CÔNG CỤ CHỤP MÀN HÌNH NATIVE",
    "hero.w1": "Chọn.",
    "hero.w2": "Chụp.",
    "hero.w3": "Chia sẻ.",
    "hero.sub": "Chụp vùng màn hình chuẩn từng pixel trên mọi màn hình, kèm chú thích và OCR native. Thuần Rust, không Electron.",
    "hero.gh": "Xem trên GitHub",

    "strip.1": "CHUẨN RETINA",
    "strip.2": "ĐA MÀN HÌNH",
    "strip.3": "CHÚ THÍCH",
    "strip.4": "OCR NATIVE",
    "strip.5": "KHÔNG ELECTRON",
    "strip.6": "THUẦN RUST",

    "sh.eyebrow": "TRÌNH DIỄN",
    "sh.h2": "Trọn vẹn quy trình, trên màn hình.",
    "sh.t1": "DEMO",
    "sh.t2": "LỚP PHỦ",
    "sh.t3": "CHÚ THÍCH",
    "sh.t4": "TRAY",
    "sh.t5": "CÀI ĐẶT",

    "feat.h2": "Đủ mọi thứ một cú chụp cần.<br>Không thừa một thứ nào.",
    "f1.h": "Chú thích ngay tại chỗ",
    "f1.p": "Vẽ trực tiếp lên lớp phủ làm mờ — bút, mũi tên, hình chữ nhật, ellipse, bút dạ quang, chữ, làm nhòe vùng — mọi thứ gói gọn trong vùng đã chụp. Công cụ chữ là một hộp soạn thảo thật: cỡ chữ (12–48), bật/tắt đậm, con trỏ, bôi chọn và ⌘A, gõ tiếng Việt qua IME. Mọi hình và khối chữ đều chỉnh được — công cụ Select nhấc lên, kéo đi chỗ khác, hoặc xoá. Nét vẽ nằm trên lớp độ phân giải vật lý nên sắc nét như ảnh gốc. Undo và redo chỉ một cú click.",
    "f2.h": "Phím tắt toàn cục, nằm sẵn trên tray",
    "f2.p": "Quickshort nằm trong menu bar. Nhấn ⌘⇧S ở bất cứ đâu là lớp phủ làm mờ phủ kín mọi màn hình cùng lúc. Esc để huỷ, và phím tắt ghi lại được trong Cài đặt.",
    "f3.h": "Luôn là pixel vật lý",
    "f3.p": "Ảnh chụp lấy độ phân giải framebuffer, không phải point logic. Trên màn Retina, PNG rộng 2880px và chữ sắc nét ở zoom 100%. Hệ thống nhiều DPI map từng màn theo hệ số scale riêng của nó.",
    "f4.h": "OCR bằng engine của OS",
    "f4.p": "⌘T đưa vùng chọn qua macOS Vision (Windows.Media.Ocr trên Windows) và bỏ văn bản vào clipboard kèm toast xác nhận. Engine native, không model đóng gói, không gọi cloud.",
    "f5.h": "Chọn xuyên màn hình",
    "f5.p": "Bắt đầu kéo trên một màn và kết thúc ở màn khác. Phần crop của mỗi màn được map qua origin và DPI riêng, rồi ghép thành một ảnh duy nhất.",
    "f6.h": "Link share sống lâu hơn app",
    "f6.p": "⌘L upload ảnh lên Cloudflare edge worker và bỏ link công khai vào clipboard — vẫn trả ảnh 7 ngày sau đó, lâu sau khi bạn thoát app.",
    "f7.h": "Local-first, không tài khoản",
    "f7.p": "Không đăng ký, không telemetry, không gì rời máy bạn. Ảnh nằm trong thư mục của bạn, cấu hình là một file config thuần, và share đi qua một relay tối thiểu — không storage của bên thứ ba.",
    "f8.h": "One-shot chạy được trong script",
    "f8.p": "<code>quickshort --capture --out shot.png</code> bỏ qua vòng đời tray — chọn, xong, thoát. <code>--delay</code> chờ menu và tooltip, nên kết hợp được với launchd, CI, hay shell alias.",
    "f9.h": "Tự cập nhật",
    "f9.p": "Tray kiểm tra GitHub Releases mỗi ngày một lần. Bản cập nhật tải .app mới, kiểm SHA-256, thay thế tại chỗ kèm rollback nếu lỗi, rồi khởi động lại. Chữ ký self-signed ổn định giữ nguyên quyền Screen Recording và Accessibility.",
    "f10.h": "Quay màn hình",
    "f10.p": "⌘R biến vùng chọn thành mp4 — PipeWire trên Wayland, ScreenCaptureKit trên macOS, GDI trên Windows. ffmpeg encode trên thread riêng ở 15fps theo đồng hồ thật, nên encode chậm chỉ drop frame chứ không làm clip ngắn lại. Thẻ REC nổi dừng quay.",
    "f11.h": "Chụp cuộn trang",
    "f11.p": "⌘G chụp lặp lại vùng chọn trong lúc bạn cuộn rồi ghép các khung thành một ảnh dài — tài liệu, thread hay log chat không bao giờ vừa một màn hình.",
    "f12.h": "Pin & làm đẹp",
    "f12.p": "Pin ghim ảnh luôn nổi trên cùng làm mẫu tham chiếu, kéo đi đâu cũng được. Beautify gắn ảnh lên nền gradient mềm với bo góc và đổ bóng — sẵn sàng cho docs và bài đăng.",

    "pipe.eyebrow": "KIẾN TRÚC",
    "pipe.h2": "Bốn tầng. Không trình duyệt.",
    "p1.h": "Chụp",
    "p1.p": "Mọi màn hình vào RAM trong một lượt. ScreenCaptureKit, GDI BitBlt, hoặc PipeWire shm.",
    "p2.h": "Lớp phủ",
    "p2.p": "Cửa sổ borderless trên mỗi màn làm mờ khung hình và bắt thao tác kéo theo toạ độ ảo.",
    "p3.h": "Map",
    "p3.p": "Toạ độ vùng chọn quy đổi qua origin và hệ số DPI của từng màn.",
    "p4.h": "Ghép",
    "p4.p": "Crop từng màn gộp lại trên worker thread trong khi event loop vẫn rảnh.",

    "dl.h2": "Tải ngay.",
    "dl.mac": "Phát hiện macOS. DMG hoặc Homebrew.",
    "dl.win": "Phát hiện Windows. Một file setup exe.",
    "dl.linux": "Phát hiện Linux. AppImage hoặc .deb.",
    "dl.cta": "Tải cho {os}",
    "dl.m1.d": "Tải về, mở, kéo vào Applications. Build chưa ký: chuột phải → Open, một lần.",
    "dl.m1.b": "Bản mới nhất",
    "dl.m2.d": "Tap riêng, đã gỡ quarantine sẵn.",
    "dl.w1.d": "Setup wizard: cài vào Program Files, shortcut Start Menu, kèm uninstaller. Build chưa ký: SmartScreen cảnh báo một lần.",
    "dl.w1.b": "Tải .exe",
    "dl.w2.d": "Tự build cho chắc? GDI capture và Windows.Media.Ocr đều native.",
    "dl.l1.d": "Portable, không cần cài. Tải về, chmod +x, chạy. Kèm sẵn dependency cho tray.",
    "dl.l1.b": "Tải AppImage",
    "dl.l2.d": "Đăng ký .desktop entry và icon, vào /usr/bin.",
    "dl.perm": "macOS 14+ cần quyền Screen Recording và Accessibility. Bộ cài Windows chưa ký (SmartScreen một lần). OCR trên Linux chạy Tesseract với tự nhận diện script. Quay màn hình (⌘R) cần ffmpeg trong PATH — brew, winget hoặc apt.",

    "req.eyebrow": "GÓP Ý",
    "req.h2": "Định hình tính năng tiếp theo.",
    "req.desc": "Báo lỗi gửi thẳng từ app — ý tưởng gửi từ đây. Cho mình biết Quickshort nên học thêm gì. Không cần tài khoản; bên trong nó vẫn là một GitHub issue.",
    "req.title": "Tiêu đề ngắn — vd: “Ghim ảnh chụp lên màn hình”",
    "req.note": "Tính năng nên làm gì? Nó giúp gì cho workflow của bạn?",
    "req.contact": "Liên hệ (không bắt buộc — GitHub @handle hoặc email)",
    "req.send": "Gửi đề xuất",
    "req.sending": "Đang gửi…",
    "req.sent": "Đã gửi — theo dõi trên GitHub",
    "req.fail": "Gửi không được — thử GitHub issues",

    "donate.eyebrow": "ỦNG HỘ",
    "donate.h2": "Miễn phí cho tất cả.<br>Được nuôi bởi một vài người.",
    "donate.gh": "Quốc tế — thẻ hoặc PayPal. Sponsor cá nhân không mất phí.",
    "donate.gh.b": "Trở thành sponsor",
    "donate.qr": "Trong nước — quét bằng app ngân hàng bất kỳ, tiền vào thẳng tài khoản.",

    "sup.title": "CẢM ƠN",
    "sup.empty": "Hãy là cái tên đầu tiên trên bảng này.",
    "sup.name": "Tên của bạn (hiển thị công khai)",
    "sup.send": "Đưa tôi lên",
    "sup.sending": "Đang gửi…",
    "sup.sent": "Đã nhận — hẹn gặp trên bảng",
    "sup.fail": "Gửi không được",
    "sup.hint": "Đã chuyển khoản qua VietQR? Để lại tên — tên bạn sẽ lên đây sau khi tiền về.",

    "ui.copy": "Chép",
    "ui.copied": "Đã chép",
    "ui.failed": "Lỗi",
    "os.macos": "macOS",
    "os.windows": "Windows",
    "os.linux": "Linux",
  },
};

window.QS_T = (k, vars) => {
  const lang = document.documentElement.dataset.lang || "en";
  let s = (window.QS_I18N[lang] && window.QS_I18N[lang][k]) || window.QS_I18N.en[k] || k;
  if (vars) for (const [n, v] of Object.entries(vars)) s = s.replace(`{${n}}`, v);
  return s;
};

window.QS_APPLY_LANG = (lang) => {
  if (!window.QS_I18N[lang]) lang = "en";
  document.documentElement.dataset.lang = lang;
  document.documentElement.lang = lang;
  document.title = window.QS_T("meta.title");
  document.querySelector('meta[name="description"]')?.setAttribute("content", window.QS_T("meta.desc"));
  document.querySelectorAll("[data-i18n]").forEach((el) => {
    el.innerHTML = window.QS_T(el.dataset.i18n);
  });
  document.querySelectorAll("[data-i18n-ph]").forEach((el) => {
    el.placeholder = window.QS_T(el.dataset.i18nPh);
  });
  document.querySelectorAll(".lang-sw").forEach((el) => {
    el.classList.toggle("on", el.dataset.lang === lang);
  });
  document.dispatchEvent(new CustomEvent("qs-lang", { detail: lang }));
};
