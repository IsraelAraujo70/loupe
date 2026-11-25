from slug import slugify


def test_accents_and_punctuation_become_single_dashes():
    assert slugify("Ação rápida: já!") == "acao-rapida-ja"


def test_long_titles_stop_at_a_word_boundary():
    slug = slugify("one two three four five six seven eight nine ten eleven twelve")
    assert len(slug) <= 48
    assert not slug.endswith("-")


def test_a_title_without_words_is_untitled():
    assert slugify("!!!") == "untitled"
