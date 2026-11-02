from deck_sim.webcam import fit_width


def test_scales_down_keeping_aspect_and_even_dims():
    assert fit_width(640, 480, 480) == (480, 360)
    assert fit_width(1280, 720, 480) == (480, 270)
    assert fit_width(1920, 1080, 480) == (480, 270)


def test_never_scales_up_and_zero_means_no_limit():
    assert fit_width(320, 240, 480) == (320, 240)
    assert fit_width(641, 481, 0) == (640, 480)
