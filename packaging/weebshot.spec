# Binary release package. scripts/package-rpm.sh stages the matching AppImage
# payload and supplies weebshot_version, appdir, and the private-library filter.
%global debug_package %{nil}
%global _build_id_links none
# Preserve the already-built release payload, including its relative RUNPATHs.
%global __os_install_post %{nil}
%global __provides_exclude_from ^%{_libdir}/weebshot/.*$

Name:           weebshot
Version:        %{weebshot_version}
Release:        1
Summary:        Frame-accurate anime screenshots, clips, and stitched artwork
License:        GPL-3.0-or-later
URL:            https://github.com/Melanpan/weebshot
BuildArch:      x86_64
Requires:       hicolor-icon-theme
# Window backends load these at runtime, so ELF dependency scanning cannot see them.
Requires:       libwayland-client.so.0()(64bit)
Requires:       libwayland-cursor.so.0()(64bit)
Requires:       libwayland-egl.so.1()(64bit)
Requires:       libxkbcommon.so.0()(64bit)
Requires:       libxkbcommon-x11.so.0()(64bit)
Requires:       libX11.so.6()(64bit)
Requires:       libXcursor.so.1()(64bit)
Requires:       libXrandr.so.2()(64bit)
Requires:       libXi.so.6()(64bit)
Requires:       libEGL.so.1()(64bit)
Requires:       libGL.so.1()(64bit)

%description
Weebshot makes frame-accurate anime screenshots and video clips, reconstructs
panning artwork with Auto Stitch, and offers Anime4K 2x upscaling.

This upstream binary RPM installs the bundled release runtime in a private
application directory, with a desktop menu entry and a command-line launcher.
It includes libmpv, FFmpeg, gifsicle, and the Anime4K helper. Updates are installed
with dnf; the application does not replace RPM-managed files itself.

%install
mkdir -p %{buildroot}%{_libdir}/weebshot %{buildroot}%{_bindir}
cp -a %{appdir}/. %{buildroot}%{_libdir}/weebshot/
cat > %{buildroot}%{_bindir}/weebshot <<'LAUNCHER'
#!/bin/sh
unset APPIMAGE APPDIR ARGV0
exec %{_libdir}/weebshot/AppRun "$@"
LAUNCHER
chmod 0755 %{buildroot}%{_bindir}/weebshot
install -Dm0644 %{appdir}/weebshot.desktop %{buildroot}%{_datadir}/applications/weebshot.desktop
install -Dm0644 %{appdir}/weebshot.png %{buildroot}%{_datadir}/icons/hicolor/256x256/apps/weebshot.png
install -Dm0644 %{appdir}/usr/share/doc/weebshot/LICENSE %{buildroot}%{_datadir}/licenses/weebshot/LICENSE
install -Dm0644 %{appdir}/usr/share/doc/weebshot/THIRD-PARTY.md %{buildroot}%{_datadir}/doc/weebshot/THIRD-PARTY.md

%files
%{_bindir}/weebshot
%{_libdir}/weebshot/
%{_datadir}/applications/weebshot.desktop
%{_datadir}/icons/hicolor/256x256/apps/weebshot.png
%license %{_datadir}/licenses/weebshot/
%doc %{_datadir}/doc/weebshot/

%changelog
* Wed Nov 11 2026 Weebshot contributors - 0.4.0-1
- Initial upstream binary RPM with bundled runtime and desktop integration.
