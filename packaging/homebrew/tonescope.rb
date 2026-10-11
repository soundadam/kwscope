# frozen_string_literal: true

# release workflow 的 homebrew job 把 @VERSION@ 与两处 SHA-256 填好，写到 soundadam/homebrew-tap 的 Casks/tonescope.rb
cask "tonescope" do
  version "@VERSION@"

  on_macos do
    sha256 "@MACOS_SHA256@"

    url "https://github.com/soundadam/tonescope/releases/download/v#{version}/tonescope-#{version}-macos-universal.zip"

    app "Tonescope.app"
    binary "#{appdir}/Tonescope.app/Contents/MacOS/tonescope"

    # ad-hoc 签名、未公证：去掉隔离属性，与自更新换上的文件一样不被 Gatekeeper 拦下
    postflight_steps do
      run "/usr/bin/xattr", args: ["-dr", "com.apple.quarantine", "{{appdir}}/Tonescope.app"]
    end

    caveats "macOS asks for microphone access the first time Tonescope runs."
  end
  on_linux do
    on_intel do
      sha256 "@LINUX_X86_64_SHA256@"

      url "https://github.com/soundadam/tonescope/releases/download/v#{version}/tonescope-#{version}-linux-x86_64.tar.gz"

      binary "tonescope-#{version}-linux-x86_64/tonescope"

      caveats <<~EOS
        Tonescope captures audio through ALSA (libasound2); PulseAudio and
        PipeWire are reached through their ALSA compatibility layer.
      EOS
    end
  end

  name "Tonescope"
  desc "Real-time log-frequency spectrogram with musical scale overlays"
  homepage "https://soundadam.com/projects/tonescope/"

  livecheck do
    url :url
    strategy :github_latest
  end
end
