from deck_sim.test_camera import draw_pattern


def test_pattern_shape_and_motion():
    a = draw_pattern(640, 480, 1, 0.10)
    b = draw_pattern(640, 480, 2, 0.60)
    assert a.shape == (480, 640, 3)
    assert a.dtype.name == 'uint8'
    # The sweeping bar moved, so the frames differ.
    assert (a != b).any()
