//! CLDR plural categories of whole numbers for the six languages (the interface uses
//! `Intl.PluralRules`; these rules give the same answers for the same numbers).

use super::Lang;

/// The categories a plural entry of `lang` has to give (always ending with `other`).
pub fn required_categories(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::En | Lang::De => &["one", "other"],
        Lang::Es | Lang::Fr => &["one", "many", "other"],
        Lang::Pl => &["one", "few", "many", "other"],
        Lang::Ja => &["other"],
    }
}

/// CLDR category of the whole number `n` in `lang`: `one`, `few`, `many` or `other`.
pub fn plural_category(lang: Lang, n: i64) -> &'static str {
    let i = n.unsigned_abs();
    // Spanish and French: whole millions (1 000 000, 2 000 000, ...) are `many`.
    let millions = i != 0 && i.is_multiple_of(1_000_000);
    match lang {
        Lang::En | Lang::De => {
            if i == 1 {
                "one"
            } else {
                "other"
            }
        }
        Lang::Es => {
            if i == 1 {
                "one"
            } else if millions {
                "many"
            } else {
                "other"
            }
        }
        Lang::Fr => {
            if i <= 1 {
                "one"
            } else if millions {
                "many"
            } else {
                "other"
            }
        }
        Lang::Pl => {
            let (m10, m100) = (i % 10, i % 100);
            if i == 1 {
                "one"
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                "few"
            } else {
                "many"
            }
        }
        Lang::Ja => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn categories(lang: Lang, numbers: &[i64]) -> Vec<&'static str> {
        numbers.iter().map(|&n| plural_category(lang, n)).collect()
    }

    #[test]
    fn polish_one_few_many() {
        assert_eq!(
            categories(Lang::Pl, &[1, 2, 5, 12, 22, 25]),
            ["one", "few", "many", "many", "few", "many"]
        );
        assert_eq!(
            categories(Lang::Pl, &[0, 3, 4, 11, 13, 14, 21, 102, 112, 1001]),
            [
                "many", "few", "few", "many", "many", "many", "many", "few", "many", "many"
            ]
        );
    }

    #[test]
    fn french_zero_and_one_are_one() {
        assert_eq!(categories(Lang::Fr, &[0, 1, 2]), ["one", "one", "other"]);
        assert_eq!(
            categories(Lang::Fr, &[1_000_000, 1_000_001]),
            ["many", "other"]
        );
    }

    #[test]
    fn english_german_and_spanish() {
        assert_eq!(categories(Lang::En, &[1, 2, 0]), ["one", "other", "other"]);
        assert_eq!(categories(Lang::De, &[1, 2, 0]), ["one", "other", "other"]);
        assert_eq!(
            categories(Lang::Es, &[1, 0, 2, 1_000_000]),
            ["one", "other", "other", "many"]
        );
    }

    #[test]
    fn japanese_has_only_other() {
        assert_eq!(categories(Lang::Ja, &[0, 1, 2, 5]), ["other"; 4]);
    }

    #[test]
    fn negative_numbers_count_like_positive_ones() {
        assert_eq!(plural_category(Lang::Pl, -2), "few");
        assert_eq!(plural_category(Lang::En, -1), "one");
    }

    #[test]
    fn every_category_a_rule_gives_is_required() {
        for lang in Lang::ALL {
            for n in (0..=2000).chain([1_000_000, 1_000_001, 2_000_000]) {
                let category = plural_category(lang, n);
                assert!(
                    required_categories(lang).contains(&category),
                    "{lang:?} {n}: {category}"
                );
            }
        }
    }
}
