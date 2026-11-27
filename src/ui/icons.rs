// Copyright (c) Vitaly Golik (mjojo)
// SPDX-License-Identifier: MIT OR Apache-2.0

use hey_widgets::Env;

const LOGO_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64">
  <defs>
    <linearGradient id="gArrow" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0%" stop-color="#FFC45E"/>
      <stop offset="55%" stop-color="#F5A623"/>
      <stop offset="100%" stop-color="#D47E00"/>
    </linearGradient>
    <linearGradient id="gCyan" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0%" stop-color="#4DF3FF"/>
      <stop offset="100%" stop-color="#00C8E6"/>
    </linearGradient>
    <linearGradient id="gAmber" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0%" stop-color="#FFC254"/>
      <stop offset="100%" stop-color="#E08B08"/>
    </linearGradient>
    <linearGradient id="gDark" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0%" stop-color="#222630"/>
      <stop offset="100%" stop-color="#14171E"/>
    </linearGradient>
  </defs>
  <!-- Downward beveled arrow -->
  <path d="M25 5 H39 V22 H49 L32 38 L15 22 H25 Z" fill="url(#gArrow)" stroke="#8F5300" stroke-width="1.6" stroke-linejoin="miter"/>
  <path d="M26.5 6.8 H37.5 M18.5 23.2 H26 M38 23.2 H45.5" stroke="#FFE6B0" stroke-width="1.3" stroke-linecap="square" opacity="0.8"/>
  <!-- 3x2 piece grid -->
  <!-- Row 1: Cyan, Cyan, Amber -->
  <rect x="12" y="40" width="12" height="8" fill="url(#gCyan)" stroke="#4A5260" stroke-width="1.3"/>
  <rect x="26" y="40" width="12" height="8" fill="url(#gCyan)" stroke="#4A5260" stroke-width="1.3"/>
  <rect x="40" y="40" width="12" height="8" fill="url(#gAmber)" stroke="#4A5260" stroke-width="1.3"/>
  <!-- Row 2: Cyan, Dark, Dark -->
  <rect x="12" y="50" width="12" height="8" fill="url(#gCyan)" stroke="#4A5260" stroke-width="1.3"/>
  <rect x="26" y="50" width="12" height="8" fill="url(#gDark)" stroke="#4A5260" stroke-width="1.3"/>
  <rect x="40" y="50" width="12" height="8" fill="url(#gDark)" stroke="#4A5260" stroke-width="1.3"/>
</svg>"##;

const PLUS_SVG: &[u8] =
    br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">
  <path d="M12 4V20M4 12H20" stroke="#FFFFFF" stroke-width="2.4" stroke-linecap="square"/>
</svg>"##;

const PAUSE_SVG: &[u8] =
    br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">
  <rect x="6" y="5" width="4" height="14" fill="#FFFFFF"/>
  <rect x="14" y="5" width="4" height="14" fill="#FFFFFF"/>
</svg>"##;

const PLAY_SVG: &[u8] =
    br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">
  <polygon points="7,5 19,12 7,19" fill="#FFFFFF"/>
</svg>"##;

const TRASH_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">
  <path d="M4 7H20M9 7V4H15V7M6 7L7 20H17L18 7M10 11V16M14 11V16" fill="none" stroke="#FFFFFF" stroke-width="2" stroke-linejoin="miter"/>
</svg>"##;

const SEARCH_SVG: &[u8] =
    br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">
  <circle cx="10.5" cy="10.5" r="6" fill="none" stroke="#FFFFFF" stroke-width="2"/>
  <path d="M15 15L20 20" stroke="#FFFFFF" stroke-width="2.2" stroke-linecap="square"/>
</svg>"##;

const INFO_SVG: &[u8] =
    br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">
  <circle cx="12" cy="12" r="9" fill="none" stroke="#FFFFFF" stroke-width="2"/>
  <circle cx="12" cy="8" r="1.2" fill="#FFFFFF"/>
  <path d="M12 11.2V16.8" stroke="#FFFFFF" stroke-width="2.2" stroke-linecap="square"/>
</svg>"##;

const GEAR_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">
  <circle cx="12" cy="12" r="3.4" fill="none" stroke="#FFFFFF" stroke-width="2"/>
  <path d="M12 2.5V5.2M12 18.8V21.5M2.5 12H5.2M18.8 12H21.5M5.3 5.3L7.2 7.2M16.8 16.8L18.7 18.7M18.7 5.3L16.8 7.2M7.2 16.8L5.3 18.7" stroke="#FFFFFF" stroke-width="2" stroke-linecap="square"/>
  <circle cx="12" cy="12" r="6.8" fill="none" stroke="#FFFFFF" stroke-width="1.8"/>
</svg>"##;

pub fn ensure_icons(env: &Env) {
    let mut host = env.icons.borrow_mut();
    if host.cache.contains("tortor_logo") {
        return;
    }
    host.cache.add_source("tortor_logo", LOGO_SVG);
    host.cache.add_source("plus", PLUS_SVG);
    host.cache.add_source("pause", PAUSE_SVG);
    host.cache.add_source("play", PLAY_SVG);
    host.cache.add_source("trash", TRASH_SVG);
    host.cache.add_source("search", SEARCH_SVG);
    host.cache.add_source("info", INFO_SVG);
    host.cache.add_source("settings", GEAR_SVG);
}
