//! A throw-away 2048-bit RSA key (PKCS#8, DER, base64) generated once for the tests of the FCM
//! sender. It protects nothing and signs nothing outside the tests; it is kept without PEM
//! header on purpose, the tests put the header back.

pub const TEST_RSA_PKCS8_B64: [&str; 26] = [
    "MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCgCbjOPeb/rAOa",
    "u4726WBv1UonvtlVuZ8415Z+fewn+3q9Ua1UeRj77nyZ6zCDHqB0RGkRQTLaSUnr",
    "W9xKwNiM93sXuplsXffkeL4YCUceSC3S4kq2LqQogT4xM1CwSWGLBIOCvdVXKdtf",
    "qgbaAgfHNpvxI6BEENvEoa+q7pX5IXxNhkLDjHrUEPF6pdpf7AXPFg0G7Fgzm4R8",
    "90sSXsQckaSSzF/hRJmEAb0bXlFDli77AiOan16bjhFcP20SQ042EjjUkVkad0jD",
    "GOrsDdpQDgbDUxy8k5ZzCeQ7dtlq8FE+EDflxrAvjQvcGhbrytx5OjBpzX8a/BYr",
    "9nHijPCBAgMBAAECggEADPlcyxOHo5iNXkDwUSIQWGChN2GoeuOmfdQ0diK9P2yK",
    "wchaVevPU7U80rmXYDneZi6ZmfWAKuQgIv16eXDy3VK+3uiYwYlIrHQBbS9rXZAa",
    "bus7hVN4qXP418JCO1hnmP6Prsy2++7/wtyk242olcTelYj+2CXJu0IkXKYk8GJe",
    "vZpSvlf2eOSf+XyvoDbti6wKNePxsb9H0VFV19NCEOfac/xWgJeEPTMan+IOHnhG",
    "pKqprkNwSN7DP+LQZSkVmHzIcfN/QL4bDbWF3iOaY3fmJDZxHCIql+aSVwyEocdk",
    "A6xmTc1ObPB4jyzFdBnBJacPrfKx95L1MJh7bqGGRQKBgQDQFPbQ2Bz30ZD/Po8n",
    "ZDwRyXYzm4WyrbJSIGYNHxyecsNiH/HkncgSJ68mnvMqqgKwPxrpwjIUX625S7Od",
    "iOlo6i+537OQODRNoyxNyGqb4lPTwqvz45DKL4k1Wj97Jukj4PhQ5Yg1BQpFQl+2",
    "eJrb440hl5eqOc+4MNSiEU8yxwKBgQDE5G2zINZYOsYPVRBuCX1GiCA3YZBkx3Ek",
    "12FW8yg6wocfhfG7D+3RGGpNfQMfQC0lvdDaG6MuXmv5fziVefUbbWcBHQ3pDxp3",
    "4ZhQD8Nnt/bt8y599ehoCdGewams4yEofnb4Sgg3Hmv1+228AVHEYjSt06+YimXo",
    "6oZGAar6dwKBgDX8KmjWmPVmppLCPUvAAqGuvf73q4FG9Uu/YhqnZrppFe3gUda5",
    "ZM3pA9UmSfpYr2fgh2/8VRfaFu1LCaemfMAeehFkS2bwXgmCex0XESmuV4dKK2iU",
    "Mt1x5cw0xOGDn9sSI/qBAoyW6So85S4nLi48ZEJB3TRp2U11RcVibkTDAoGAYRuU",
    "q0tfzCY4tykqMEtH2TicQxRAhiQDvrrnjF3jEpAM2tRSNAOYNI+JJmyo1InvenTT",
    "BC+rXNw+/s/rVXXQGNXtfpQY30COul+uopYTtxz5kcRlwsQ8+ssU8urrR5TCLtmo",
    "za6dvKmpb6mxJo+0B1q0RqNoMKDZwHWjAfYioOMCgYAfOcCJcfwyYdM1dtXZ0duh",
    "r+WPanxrb4hMq+vGCmIYFjiYRHjJHvh9RKR45n6FamEt74+kdChVWk0F5/LshsA9",
    "8o9f21OeKafk4goUk2Q5zQ3TBCpIUXhgrUu5cQ0T1rCUeMVelguS6DAgF6r5eSKL",
    "RzGqSN+t1tSWeeeGvXIRuA==",
];

/// The service-account JSON the tests give to the FCM sender.
pub fn service_account_json(token_uri: &str) -> String {
    let body: String = TEST_RSA_PKCS8_B64.concat();
    let pem = format!("-----BEGIN PRIVATE KEY-----\n{body}\n-----END PRIVATE KEY-----\n");
    serde_json::json!({
        "type": "service_account",
        "project_id": "tomi-test",
        "private_key_id": "test",
        "private_key": pem,
        "client_email": "relay@tomi-test.iam.gserviceaccount.com",
        "client_id": "1",
        "token_uri": token_uri,
    })
    .to_string()
}
