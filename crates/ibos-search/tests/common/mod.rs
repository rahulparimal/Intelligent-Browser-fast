//! Shared fixtures: the same 10-document corpus and gold queries used by the Python test suite.
#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use std::path::Path;

pub const CORPUS: [(&str, &str); 10] = [
    (
        "leave-policy.md",
        "# Leave Policy\n\nFull-time employees receive 25 days of paid annual leave per calendar year. Unused leave may be carried over, capped at 5 days.\n\n## Parental leave\nPrimary caregivers receive 16 weeks of paid parental leave. Secondary caregivers receive 6 weeks.",
    ),
    (
        "expenses.md",
        "# Travel & Expense Policy\n\nSubmit expense reports within 30 days of travel. Receipts are required for any purchase above $25. Economy class is required for flights under 6 hours.\n\nMileage is reimbursed at $0.67 per mile.",
    ),
    (
        "security.md",
        "# Information Security Standard\n\nAll laptops must use full-disk encryption. Passwords must be at least 14 characters. Multi-factor authentication (MFA) is mandatory for e-mail, VPN and the admin consoles.\n\nReport phishing to security@example.com immediately.",
    ),
    (
        "onboarding.html",
        "<html><head><title>New Hire Onboarding</title></head><body><h1>Welcome!</h1><p>On your first day you will receive a laptop, badge and access to Slack. Your manager will schedule a 30-60-90 day plan.</p></body></html>",
    ),
    (
        "remote-work.md",
        "# Remote Work Guidelines\n\nEmployees may work remotely up to 3 days per week with manager approval. A home-office stipend of $500 is provided once. Core collaboration hours are 10:00-15:00 local time.",
    ),
    (
        "benefits.md",
        "# Benefits Overview\n\nHealth insurance starts on the first day of employment. The company matches 401(k) contributions up to 4%. A wellness allowance of $300 per year covers gym memberships.",
    ),
    (
        "holidays.txt",
        "Company holidays 2026: New Year's Day, Memorial Day, Independence Day, Labor Day, Thanksgiving and the day after, Christmas Day. Floating holidays: 2 per year.",
    ),
    (
        "naïve-café.md",
        "# Café réunion notes\n\nThe naïve approach to résumé screening was replaced. Zoë and José maintain the façade of the process.",
    ),
    (
        "code-of-conduct.md",
        "# Code of Conduct\n\nWe treat each other with respect. Harassment of any kind is not tolerated. Report concerns to HR or the ethics hotline; retaliation is prohibited.",
    ),
    (
        "faq.md",
        "# FAQ\n\nHow do I reset my password? Use the self-service portal. How many vacation days do I get? See the leave policy. Who approves travel? Your manager.",
    ),
];

/// (query, title of the document that must rank first)
pub const GOLD: [(&str, &str); 20] = [
    ("annual leave", "leave-policy"),
    ("what is the parental leave policy", "leave-policy"),
    ("email", "security"),
    ("e-mail MFA", "security"),
    ("$25 receipt", "expenses"),
    ("401(k) match", "benefits"),
    ("naive cafe resume", "naïve-café"),
    ("résumé", "naïve-café"),
    ("remote work stipend", "remote-work"),
    ("30-60-90", "New Hire Onboarding"),
    ("holiday", "holidays"),
    ("ethics hotline", "code-of-conduct"),
    ("encryption", "security"),
    ("phishing", "security"),
    ("gym", "benefits"),
    ("mileage rate", "expenses"),
    ("Slack badge", "New Hire Onboarding"),
    ("how do I reset my password", "faq"),
    ("retaliation", "code-of-conduct"),
    ("vpn", "security"),
];

pub fn write_corpus(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    for (name, text) in CORPUS {
        std::fs::write(dir.join(name), text).unwrap();
    }
}
