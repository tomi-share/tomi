//! Simplified Chinese (mainland usage). Informal "你"; "隐藏" / "显示" (never "秘密" / "揭示").

use super::Table;

pub const ZH: Table = Table {
    tray_open: "打开",
    tray_history: "历史记录",
    tray_settings: "设置",
    tray_quit: "退出",
    toast_copy: "复制",
    toast_open: "打开",
    tooltip_sending: "正在发送…",
    tooltip_unread_one: "{n} 条未读",
    tooltip_unread_other: "{n} 条未读",
    toast_title_link: "新链接",
    toast_title_text: "新文本",
    toast_title_image: "新图片",
    toast_title_file: "新文件",
    toast_title_files: "新文件",
    toast_title_secret: "收到隐藏内容",
    toast_title_unreadable: "来自 {device} 的条目",
    toast_body_unreadable_update: "暂时无法读取：需要更新 Tomi",
    toast_body_unreadable_other: "暂时无法读取",
    toast_title_offer: "收到历史记录提议",
    toast_from: "来自 {device}",
    toast_group_title: "{n} 个新条目",
    toast_group_body: "打开 Tomi 查看",
    toast_title_new_device: "已添加新设备",
    toast_body_new_device: "“{device}”已加入你的 Tomi 空间。不是你本人？请吊销它。",
    hello_delete_space: "删除你的 Tomi 空间。此操作无法撤销：需要进行 Windows Hello 验证。",
    hello_erase:
        "在服务器未确认的情况下抹掉这台电脑。此操作无法撤销：需要进行 Windows Hello 验证。",
    hello_revoke: "从你的 Tomi 空间中吊销一台设备。空间密钥将被更新：需要进行 Windows Hello 验证。",
    hello_reveal: "显示、复制或打开隐藏内容。需要进行 Windows Hello 验证（2 分钟内有效）。",
    hello_add_device: "向你的 Tomi 空间添加设备。它将收到空间密钥：需要进行 Windows Hello 验证。",
    account_delete_space: "删除你的 Tomi 空间。此操作无法撤销：请输入你的 Windows 帐户密码以确认。",
    account_erase:
        "在服务器未确认的情况下抹掉这台电脑。此操作无法撤销：请输入你的 Windows 帐户密码以确认。",
    account_revoke:
        "从你的 Tomi 空间中吊销一台设备。空间密钥将被更新：请输入你的 Windows 帐户密码以确认。",
    account_reveal: "显示、复制或打开隐藏内容：请输入你的 Windows 帐户密码（2 分钟内有效）。",
    account_add_device:
        "向你的 Tomi 空间添加设备。它将收到空间密钥：请输入你的 Windows 帐户密码以确认。",
    account_generic: "请输入你的 Windows 帐户密码以确认。",
};
