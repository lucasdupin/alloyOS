use metalos_boot::splash::{
    crystal_weld_alpha, ferrofluid_emblem_field, mercury_type_alpha, shade_mercury_pixel,
    wordmark_meniscus_field, LiquidSplashAnimation, Metaball, OBSIDIAN_BLACK_ARGB,
};

#[test]
fn splash_metaball_and_surface_tension_filters() {
    assert_eq!(crystal_weld_alpha(0), 0);
    assert_eq!(crystal_weld_alpha(95), 0);
    assert!(crystal_weld_alpha(105) > 0 && crystal_weld_alpha(105) < 255);
    assert_eq!(crystal_weld_alpha(115), 255);
    assert_eq!(crystal_weld_alpha(255), 255);

    assert_eq!(mercury_type_alpha(0), 0);
    assert_eq!(mercury_type_alpha(255), 255);

    let ball = Metaball::new(50, 60, 12);
    let (center_val, _, _) = ball.field_contribution(50, 60);
    assert_eq!(center_val, 255);
    let (far_val, _, _) = ball.field_contribution(100, 100);
    assert_eq!(far_val, 0);

    assert_eq!(shade_mercury_pixel(0, 0, 0), OBSIDIAN_BLACK_ARGB);
    let lit = shade_mercury_pixel(255, -24, -32);
    assert_ne!(lit, OBSIDIAN_BLACK_ARGB);
    assert_eq!(lit >> 24, 0xFF);
}

#[test]
fn splash_emblem_and_wordmark_geometry_fields() {
    // Center of Ferrofluid Crystal Emblem should be saturated
    let (core_f, _, _) = ferrofluid_emblem_field(0, 0, 256);
    assert!(core_f >= 255);

    // Top spike of Ferrofluid Crystal Emblem (0, -140) should have non-zero field
    let (spike_f, _, _) = ferrofluid_emblem_field(0, -140, 256);
    assert!(spike_f > 0);

    // Wordmark stroke and meniscus bridge positions in 960x320 SVG coordinates
    let (m_stem, _, _) = wordmark_meniscus_field(64, 170);
    assert!(m_stem > 150);
    let (bridge_me, _, _) = wordmark_meniscus_field(176, 157);
    assert!(bridge_me > 150);
}

#[test]
fn splash_animation_drip_and_coalescence_into_mercuryd_border() {
    let mut anim = LiquidSplashAnimation::new(200, 140);
    assert_eq!(anim.coalesce_progress, 0);

    let y_start = anim.droplets[0].y_q16;
    anim.step_frame(150);
    assert!(anim.droplets[0].y_q16 > y_start);
    assert!(anim.coalesce_progress > 0 && anim.coalesce_progress < 100);

    anim.step_frame(1100);
    assert_eq!(anim.coalesce_progress, 100);

    let mut fb = vec![0u32; 200 * 140];
    let dirty = anim.render_frame(&mut fb, 200);
    assert!(dirty.width > 0 && dirty.height > 0);

    let rect = anim.target_window_rect;
    let top_border_idx = (rect.y as usize) * 200 + (rect.x + rect.width / 2) as usize;
    assert_ne!(fb[top_border_idx], OBSIDIAN_BLACK_ARGB);
}
