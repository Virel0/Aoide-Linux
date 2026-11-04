//! A cover's main colours, for the moving grounds: the blurred cover's
//! pixels gathered into four groups by colour (k-means, a few rounds), each
//! group's average one colour, the largest group first.

/// How many colours, and how many rounds of regrouping.
const COLOURS: usize = 4;
const ROUNDS: usize = 8;

/// The main colours of `pixels` (linear light), most of the picture first.
/// Black for a picture with no pixels.
pub fn main_colours(pixels: &[[f32; 3]]) -> [[f32; 3]; COLOURS] {
    if pixels.is_empty() {
        return [[0.0; 3]; COLOURS];
    }
    let mut centres = starting_centres(pixels);
    let mut counts = [0_usize; COLOURS];
    for _ in 0..ROUNDS {
        let mut sums = [[0.0_f32; 3]; COLOURS];
        counts = [0; COLOURS];
        for pixel in pixels {
            let group = nearest(&centres, *pixel);
            counts[group] += 1;
            for (sum, value) in sums[group].iter_mut().zip(pixel) {
                *sum += value;
            }
        }
        for (centre, (sum, &count)) in centres.iter_mut().zip(sums.iter().zip(&counts)) {
            // An empty group keeps where it was.
            if count > 0 {
                *centre = sum.map(|s| s / count as f32);
            }
        }
    }
    let mut order: Vec<usize> = (0..COLOURS).collect();
    order.sort_by_key(|&group| std::cmp::Reverse(counts[group]));
    std::array::from_fn(|i| centres[order[i]])
}

/// Spread over the picture's range of light: its pixels at an eighth, three
/// eighths, five and seven, darkest to lightest.
fn starting_centres(pixels: &[[f32; 3]]) -> [[f32; 3]; COLOURS] {
    let mut by_light: Vec<&[f32; 3]> = pixels.iter().collect();
    by_light.sort_by(|a, b| light(**a).total_cmp(&light(**b)));
    std::array::from_fn(|i| *by_light[(by_light.len() * (2 * i + 1)) / (2 * COLOURS)])
}

fn nearest(centres: &[[f32; 3]; COLOURS], pixel: [f32; 3]) -> usize {
    let distance = |c: &[f32; 3]| {
        c.iter()
            .zip(pixel)
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f32>()
    };
    (0..COLOURS)
        .min_by(|&a, &b| distance(&centres[a]).total_cmp(&distance(&centres[b])))
        .unwrap_or(0)
}

fn light([r, g, b]: [f32; 3]) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.02)
    }

    #[test]
    fn the_colour_covering_most_comes_first() {
        let red = [0.8, 0.05, 0.05];
        let blue = [0.05, 0.1, 0.7];
        let mut pixels = vec![red; 750];
        pixels.extend(vec![blue; 250]);
        let colours = main_colours(&pixels);
        assert!(close(colours[0], red), "{colours:?}");
        assert!(colours.iter().any(|c| close(*c, blue)), "{colours:?}");
    }

    #[test]
    fn one_colour_gives_that_colour() {
        let teal = [0.02, 0.3, 0.3];
        assert!(
            main_colours(&vec![teal; 100])
                .iter()
                .all(|c| close(*c, teal))
        );
    }

    #[test]
    fn no_pixels_give_black() {
        assert_eq!(main_colours(&[]), [[0.0; 3]; 4]);
    }
}
