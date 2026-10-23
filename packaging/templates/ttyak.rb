# Rendered by packaging/render.py from the release's SHA256SUMS (github.com/dmytrobsp/ttyak); edit the template.
# No `version`: brew reads it from the urls (`brew audit --strict` refuses a redundant one).
class Ttyak < Formula
  desc "Slack client for the terminal, with huddles"
  homepage {{homepage}}
{{unless aarch64-linux}}
  # This release has no arm Linux archive: the Intel one here, and arm Linux refused below.
  url {{url_x86_64_linux}}
  sha256 {{sha256_x86_64_linux}}
{{end}}
  license any_of: [{{license_ids}}]

  on_macos do
    on_arm do
      url {{url_aarch64_macos}}
      sha256 {{sha256_aarch64_macos}}
    end
    on_intel do
      url {{url_x86_64_macos}}
      sha256 {{sha256_x86_64_macos}}
    end
  end

  on_linux do
{{if aarch64-linux}}
    on_arm do
      url {{url_aarch64_linux}}
      sha256 {{sha256_aarch64_linux}}
    end
    on_intel do
      url {{url_x86_64_linux}}
      sha256 {{sha256_x86_64_linux}}
    end
{{end}}
    depends_on "alsa-lib"
{{unless aarch64-linux}}
    depends_on arch: :x86_64
{{end}}
  end

  def install
    if OS.mac?
      bin.install "ttyak"
      return
    end

    # Linux: the program links libasound.so.2. The system's, set up for the desktop's sound server (PipeWire,
    # PulseAudio), comes first; Homebrew's alsa-lib only when the system has none. So no rpath (it would win): the
    # program goes in libexec and bin gets a launcher that picks.
    libexec.install "ttyak"
    arch = Hardware::CPU.arm? ? "aarch64" : "x86_64"
    cache_tag = Hardware::CPU.arm? ? "AArch64" : "x86-64"
    (bin/"ttyak").write <<~SH
      #!/bin/sh
      # ttyak's launcher from Homebrew: the system's libasound.so.2 when its loader finds one, else Homebrew's.
      exe="#{libexec}/ttyak"
      dirs="/lib/#{arch}-linux-gnu /usr/lib/#{arch}-linux-gnu /lib64 /usr/lib64"
      ldconfigs="/sbin/ldconfig /usr/sbin/ldconfig"
      tag="libasound.so.2 (libc6,#{cache_tag})"
      for dir in $dirs; do
        [ -e "$dir/libasound.so.2" ] && exec "$exe" "$@"
      done
      for ldconfig in $ldconfigs; do
        if [ -x "$ldconfig" ] && "$ldconfig" -p 2> /dev/null | grep -qF "$tag"; then
          exec "$exe" "$@"
        fi
      done
      unset ALSA_CONFIG_PATH
      LD_LIBRARY_PATH="${LD_LIBRARY_PATH:+$LD_LIBRARY_PATH:}#{formula_opt_lib("alsa-lib")}"
      export LD_LIBRARY_PATH
      exec "$exe" "$@"
    SH
    (bin/"ttyak").chmod 0755
  end

  def caveats
    <<~EOS
      ttyak keeps its settings, sign-ins, drafts and caches in your home folder, and
      `brew uninstall ttyak` leaves them there. To delete them, run this first:
        ttyak purge
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ttyak --version")
  end
end
