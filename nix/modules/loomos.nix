# services.loomos — loomos Native Host（loomosd）的 NixOS 模块。
# 声明：服务用户/组、systemd 沙箱项、设备节点访问、可变数据目录。
# 强制分工（ADR-012 §决定 4）：本模块重建系统配置；CapabilityStore 在 Native Host
# 做应用级授权；Linux 内核做内核级强制。内核缺机制时模块不虚构（如无 cgroups 就
# 不声明依赖它的沙箱项），缺失如实记录在机型文档。
{ config, lib, pkgs, ... }:
let
  cfg = config.services.loomos;
in {
  options.services.loomos = {
    enable = lib.mkEnableOption "loomos Native Host（loomosd）";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.loomos-host;
      defaultText = lib.literalExpression "pkgs.loomos-host";
      description = "loomos-host 包（nix build .#loomos-host）";
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "loomos";
      description = "服务运行身份（系统用户）";
    };
    group = lib.mkOption {
      type = lib.types.str;
      default = "loomos";
      description = "服务运行组（设备节点授权载体）";
    };

    deviceNodes = lib.mkOption {
      type = with lib.types; listOf str;
      default = [ ];
      example = [ "/dev/ttyUSB0" "/dev/dri/card0" ];
      description =
        "允许 loomosd 访问的设备节点（生成 systemd DeviceAllow 规则；对应 udev 组授权）";
    };

    audioPlayback = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "启用音频回放设备授权（/dev/snd/*）";
    };
  };

  config = lib.mkIf cfg.enable {
    users.groups.${cfg.group} = { };

    users.users.${cfg.user} = {
      isSystemUser = true;
      group = cfg.group;
      description = "loomos Native Host 服务身份";
    };

    systemd.services.loomosd = {
      description = "loomos Native Host（权威组件 + WAMR 运行时宿主）";
      wantedBy = [ "multi-user.target" ];
      after = [ "systemd-udevd.service" "systemd-modules-load.service" ];
      wants = [ "systemd-udevd.service" ];

      serviceConfig = {
        ExecStart = "${cfg.package}/bin/loomosd";
        User = cfg.user;
        Group = cfg.group;
        StateDirectory = "loomos"; # 可变数据：/var/lib/loomos（Store 外，ADR-012 §决定 7）
        Restart = "on-failure";
        RestartSec = 5;
        # Supervisor 的兜底之外再加 OS 级拉起；恢复分级决策仍在 loomos-runtime
        # ---- systemd 沙箱（声明式重建；内核支持才有效，缺机制时由机型文档记录）----
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        RestrictAddressFamilies = [ "AF_UNIX" "AF_INET" ];
        DevicePolicy = "closed";
        DeviceAllow = (map (d: "${d} rw") cfg.deviceNodes)
          ++ (lib.optionals cfg.audioPlayback [ "snd-* rw" ]);
      };
    };
  };
}
