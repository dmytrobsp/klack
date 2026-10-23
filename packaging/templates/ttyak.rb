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
    bin.install "ttyak"
    return unless OS.linux?

    # The release binary links libasound.so.2 but has no rpath. Relocate it as Homebrew relocates a poured bottle
    # (Keg#change_rpath!): Homebrew's ld.so, whose glibc the brewed alsa-lib may need, and an rpath to alsa-lib.
    ld_so = HOMEBREW_PREFIX/"lib/ld.so"
    (bin/"ttyak").patch!(interpreter: (ld_so.to_s if ld_so.readable?), rpath: formula_opt_lib("alsa-lib").to_s)
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
