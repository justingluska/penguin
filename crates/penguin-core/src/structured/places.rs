//! Airline designators and airports, for reading flight mail and for
//! questions like "my flight to Lisbon".
//!
//! Codes are IATA two-character airline designators and three-letter
//! location identifiers (IATA Airline Coding Directory / Airport Code
//! Search). This is a compact list of the airlines and airports most mail
//! mentions, not the directory: an unknown airport code is still read when
//! it sits in an explicit "SFO → LIS" pair, and an unknown airline code
//! when it follows the word "flight".

/// (IATA designator, airline name, lowercase words that identify it in a
/// sender or text).
const AIRLINES: &[(&str, &str, &[&str])] = &[
    ("AA", "American Airlines", &["american airlines", "aa.com"]),
    ("UA", "United Airlines", &["united airlines", "united.com"]),
    ("DL", "Delta Air Lines", &["delta air lines", "delta.com"]),
    (
        "WN",
        "Southwest Airlines",
        &["southwest airlines", "southwest.com"],
    ),
    ("B6", "JetBlue", &["jetblue"]),
    ("AS", "Alaska Airlines", &["alaska airlines", "alaskaair"]),
    ("NK", "Spirit Airlines", &["spirit airlines"]),
    (
        "F9",
        "Frontier Airlines",
        &["frontier airlines", "flyfrontier"],
    ),
    ("HA", "Hawaiian Airlines", &["hawaiian airlines"]),
    ("AC", "Air Canada", &["air canada", "aircanada"]),
    ("WS", "WestJet", &["westjet"]),
    ("AM", "Aeroméxico", &["aeromexico", "aeroméxico"]),
    ("Y4", "Volaris", &["volaris"]),
    ("VB", "Viva Aerobus", &["viva aerobus", "vivaaerobus"]),
    ("AV", "Avianca", &["avianca"]),
    ("LA", "LATAM Airlines", &["latam"]),
    ("CM", "Copa Airlines", &["copa airlines", "copaair"]),
    (
        "AR",
        "Aerolíneas Argentinas",
        &["aerolineas argentinas", "aerolíneas argentinas"],
    ),
    ("G3", "GOL", &["voegol", "gol linhas"]),
    ("AD", "Azul", &["voeazul", "azul linhas"]),
    (
        "BA",
        "British Airways",
        &["british airways", "britishairways", "ba.com"],
    ),
    ("VS", "Virgin Atlantic", &["virgin atlantic"]),
    ("IB", "Iberia", &["iberia"]),
    ("VY", "Vueling", &["vueling"]),
    ("UX", "Air Europa", &["air europa", "aireuropa"]),
    ("LH", "Lufthansa", &["lufthansa"]),
    ("LX", "SWISS", &["swiss international", "swiss.com"]),
    ("OS", "Austrian Airlines", &["austrian airlines"]),
    ("SN", "Brussels Airlines", &["brussels airlines"]),
    ("AF", "Air France", &["air france", "airfrance"]),
    ("KL", "KLM", &["klm"]),
    ("AZ", "ITA Airways", &["ita airways", "ita-airways"]),
    ("TP", "TAP Air Portugal", &["tap air portugal", "flytap"]),
    ("EI", "Aer Lingus", &["aer lingus", "aerlingus"]),
    ("FR", "Ryanair", &["ryanair"]),
    ("U2", "easyJet", &["easyjet"]),
    ("W6", "Wizz Air", &["wizz air", "wizzair"]),
    ("SK", "SAS", &["flysas", "scandinavian airlines"]),
    ("AY", "Finnair", &["finnair"]),
    ("DY", "Norwegian", &["norwegian air"]),
    (
        "TK",
        "Turkish Airlines",
        &["turkish airlines", "turkishairlines"],
    ),
    ("EK", "Emirates", &["emirates"]),
    ("QR", "Qatar Airways", &["qatar airways", "qatarairways"]),
    ("EY", "Etihad Airways", &["etihad"]),
    (
        "SQ",
        "Singapore Airlines",
        &["singapore airlines", "singaporeair"],
    ),
    ("CX", "Cathay Pacific", &["cathay pacific", "cathaypacific"]),
    ("JL", "Japan Airlines", &["japan airlines", "jal.com"]),
    ("NH", "ANA", &["all nippon airways", "ana.co.jp"]),
    ("KE", "Korean Air", &["korean air", "koreanair"]),
    ("OZ", "Asiana Airlines", &["asiana"]),
    ("QF", "Qantas", &["qantas"]),
    (
        "VA",
        "Virgin Australia",
        &["virgin australia", "virginaustralia"],
    ),
    (
        "NZ",
        "Air New Zealand",
        &["air new zealand", "airnewzealand"],
    ),
    ("AI", "Air India", &["air india", "airindia"]),
    ("6E", "IndiGo", &["goindigo", "indigo airlines"]),
    (
        "PR",
        "Philippine Airlines",
        &["philippine airlines", "philippineairlines"],
    ),
    ("5J", "Cebu Pacific", &["cebu pacific", "cebupacificair"]),
    (
        "CI",
        "China Airlines",
        &["china airlines", "china-airlines"],
    ),
    ("BR", "EVA Air", &["eva air", "evaair"]),
    ("CA", "Air China", &["air china", "airchina"]),
    ("MU", "China Eastern", &["china eastern", "ceair"]),
    ("CZ", "China Southern", &["china southern", "csair"]),
    (
        "ET",
        "Ethiopian Airlines",
        &["ethiopian airlines", "ethiopianairlines"],
    ),
    (
        "SA",
        "South African Airways",
        &["south african airways", "flysaa"],
    ),
    ("MS", "EgyptAir", &["egyptair"]),
    ("LY", "El Al", &["el al", "elal"]),
    ("TG", "Thai Airways", &["thai airways", "thaiairways"]),
    (
        "MH",
        "Malaysia Airlines",
        &["malaysia airlines", "malaysiaairlines"],
    ),
    (
        "GA",
        "Garuda Indonesia",
        &["garuda indonesia", "garuda-indonesia"],
    ),
    (
        "VN",
        "Vietnam Airlines",
        &["vietnam airlines", "vietnamairlines"],
    ),
];

/// (IATA code, city, extra lowercase names for the place: other spellings,
/// Spanish names, the airport's own name).
const AIRPORTS: &[(&str, &str, &[&str])] = &[
    // United States and Canada
    ("SFO", "San Francisco", &["sf", "bay area"]),
    ("OAK", "Oakland", &["bay area"]),
    ("SJC", "San Jose", &["bay area"]),
    ("LAX", "Los Angeles", &["la"]),
    ("BUR", "Burbank", &["los angeles"]),
    ("SNA", "Santa Ana", &["orange county", "john wayne"]),
    ("ONT", "Ontario", &[]),
    ("SAN", "San Diego", &[]),
    ("SMF", "Sacramento", &[]),
    ("SEA", "Seattle", &[]),
    ("PDX", "Portland", &[]),
    ("JFK", "New York", &["nyc", "nueva york", "kennedy"]),
    ("LGA", "New York", &["nyc", "nueva york", "laguardia"]),
    ("EWR", "Newark", &["new york", "nyc", "nueva york"]),
    ("BOS", "Boston", &[]),
    ("ORD", "Chicago", &["o'hare", "ohare"]),
    ("MDW", "Chicago", &["midway"]),
    ("DEN", "Denver", &[]),
    ("DFW", "Dallas", &["fort worth", "dallas fort worth"]),
    ("DAL", "Dallas", &["love field"]),
    ("IAH", "Houston", &[]),
    ("HOU", "Houston", &["hobby"]),
    ("AUS", "Austin", &[]),
    ("SAT", "San Antonio", &[]),
    ("ATL", "Atlanta", &[]),
    ("MIA", "Miami", &[]),
    ("FLL", "Fort Lauderdale", &[]),
    ("MCO", "Orlando", &[]),
    ("TPA", "Tampa", &[]),
    ("CLT", "Charlotte", &[]),
    ("PHL", "Philadelphia", &["filadelfia"]),
    ("IAD", "Washington", &["washington dc", "dc", "dulles"]),
    ("DCA", "Washington", &["washington dc", "dc", "reagan"]),
    ("BWI", "Baltimore", &[]),
    ("DTW", "Detroit", &[]),
    ("MSP", "Minneapolis", &[]),
    ("PHX", "Phoenix", &[]),
    ("LAS", "Las Vegas", &["vegas"]),
    ("SLC", "Salt Lake City", &[]),
    ("HNL", "Honolulu", &["hawaii", "oahu"]),
    ("OGG", "Kahului", &["maui"]),
    ("ANC", "Anchorage", &[]),
    ("BNA", "Nashville", &[]),
    ("MSY", "New Orleans", &["nueva orleans"]),
    ("RDU", "Raleigh", &["durham"]),
    ("STL", "St. Louis", &["saint louis"]),
    ("MCI", "Kansas City", &[]),
    ("CLE", "Cleveland", &[]),
    ("PIT", "Pittsburgh", &[]),
    ("CVG", "Cincinnati", &[]),
    ("IND", "Indianapolis", &[]),
    ("CMH", "Columbus", &[]),
    ("SJU", "San Juan", &["puerto rico"]),
    ("YYZ", "Toronto", &[]),
    ("YVR", "Vancouver", &[]),
    ("YUL", "Montreal", &["montréal"]),
    ("YYC", "Calgary", &[]),
    ("YOW", "Ottawa", &[]),
    // Mexico, Central and South America
    (
        "MEX",
        "Mexico City",
        &["cdmx", "ciudad de mexico", "ciudad de méxico"],
    ),
    ("CUN", "Cancún", &["cancun"]),
    ("GDL", "Guadalajara", &[]),
    ("MTY", "Monterrey", &[]),
    ("SJD", "Los Cabos", &["cabo", "san jose del cabo"]),
    ("PVR", "Puerto Vallarta", &["vallarta"]),
    ("BOG", "Bogotá", &["bogota"]),
    ("MDE", "Medellín", &["medellin"]),
    ("CTG", "Cartagena", &[]),
    ("LIM", "Lima", &[]),
    ("SCL", "Santiago", &["santiago de chile"]),
    ("EZE", "Buenos Aires", &["ezeiza"]),
    ("AEP", "Buenos Aires", &["aeroparque"]),
    ("GRU", "São Paulo", &["sao paulo"]),
    ("GIG", "Rio de Janeiro", &["rio"]),
    (
        "PTY",
        "Panama City",
        &["panama", "panamá", "ciudad de panama", "ciudad de panamá"],
    ),
    ("SJO", "San José", &["costa rica"]),
    ("UIO", "Quito", &[]),
    // Europe
    ("LHR", "London", &["heathrow", "londres"]),
    ("LGW", "London", &["gatwick", "londres"]),
    ("STN", "London", &["stansted", "londres"]),
    ("LTN", "London", &["luton", "londres"]),
    ("LCY", "London", &["london city", "londres"]),
    ("MAN", "Manchester", &[]),
    ("EDI", "Edinburgh", &["edimburgo"]),
    ("DUB", "Dublin", &["dublín"]),
    ("CDG", "Paris", &["parís", "charles de gaulle"]),
    ("ORY", "Paris", &["parís", "orly"]),
    ("AMS", "Amsterdam", &["ámsterdam", "schiphol"]),
    ("FRA", "Frankfurt", &["fráncfort"]),
    ("MUC", "Munich", &["münchen", "múnich"]),
    ("BER", "Berlin", &["berlín"]),
    ("ZRH", "Zurich", &["zürich", "zúrich"]),
    ("GVA", "Geneva", &["ginebra", "genève"]),
    ("VIE", "Vienna", &["viena", "wien"]),
    ("BRU", "Brussels", &["bruselas"]),
    ("CPH", "Copenhagen", &["copenhague"]),
    ("ARN", "Stockholm", &["estocolmo"]),
    ("OSL", "Oslo", &[]),
    ("HEL", "Helsinki", &[]),
    ("MAD", "Madrid", &["barajas"]),
    ("VLC", "Valencia", &["valència"]),
    ("BCN", "Barcelona", &["el prat"]),
    ("PMI", "Palma", &["mallorca", "majorca"]),
    ("AGP", "Málaga", &["malaga"]),
    ("LIS", "Lisbon", &["lisboa"]),
    ("OPO", "Porto", &["oporto"]),
    ("FCO", "Rome", &["roma", "fiumicino"]),
    ("MXP", "Milan", &["milán", "milano", "malpensa"]),
    ("LIN", "Milan", &["milán", "milano", "linate"]),
    ("VCE", "Venice", &["venecia", "venezia"]),
    ("NAP", "Naples", &["nápoles", "napoli"]),
    ("ATH", "Athens", &["atenas"]),
    ("IST", "Istanbul", &["estambul"]),
    ("PRG", "Prague", &["praga"]),
    ("WAW", "Warsaw", &["varsovia"]),
    ("BUD", "Budapest", &[]),
    // Middle East and Africa
    ("DXB", "Dubai", &["dubái"]),
    ("DOH", "Doha", &[]),
    ("AUH", "Abu Dhabi", &[]),
    ("TLV", "Tel Aviv", &[]),
    ("CAI", "Cairo", &["el cairo"]),
    ("JNB", "Johannesburg", &["johannesburgo"]),
    ("CPT", "Cape Town", &["ciudad del cabo"]),
    ("NBO", "Nairobi", &[]),
    // Asia and Oceania
    ("DEL", "Delhi", &["new delhi", "nueva delhi"]),
    ("BOM", "Mumbai", &["bombay"]),
    ("BLR", "Bengaluru", &["bangalore"]),
    ("SIN", "Singapore", &["singapur", "changi"]),
    ("HKG", "Hong Kong", &[]),
    ("NRT", "Tokyo", &["tokio", "narita"]),
    ("HND", "Tokyo", &["tokio", "haneda"]),
    ("KIX", "Osaka", &[]),
    ("ICN", "Seoul", &["seúl", "incheon"]),
    ("PEK", "Beijing", &["pekín", "pekin"]),
    ("PVG", "Shanghai", &["shanghái", "pudong"]),
    ("TPE", "Taipei", &["taipéi"]),
    ("BKK", "Bangkok", &["bangkok"]),
    ("KUL", "Kuala Lumpur", &[]),
    ("CGK", "Jakarta", &["yakarta"]),
    ("DPS", "Denpasar", &["bali"]),
    ("MNL", "Manila", &[]),
    ("CEB", "Cebu", &[]),
    ("SYD", "Sydney", &["sídney"]),
    ("MEL", "Melbourne", &[]),
    ("BNE", "Brisbane", &[]),
    ("AKL", "Auckland", &[]),
];

/// The airline with this IATA designator: (code, name).
pub(crate) fn airline_by_code(code: &str) -> Option<(&'static str, &'static str)> {
    AIRLINES
        .iter()
        .find(|(c, _, _)| c.eq_ignore_ascii_case(code))
        .map(|(c, n, _)| (*c, *n))
}

/// An airline named in `text` (lowercase): (code, name).
pub(crate) fn airline_in(text: &str) -> Option<(&'static str, &'static str)> {
    AIRLINES
        .iter()
        .find(|(_, _, words)| words.iter().any(|w| contains_word(text, w)))
        .map(|(c, n, _)| (*c, *n))
}

/// A known airport: (code, city).
pub(crate) fn airport_by_code(code: &str) -> Option<(&'static str, &'static str)> {
    AIRPORTS
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(c, n, _)| (*c, *n))
}

/// The city and other names of a known airport's place ("LIS" → Lisbon,
/// lisboa).
pub(crate) fn airport_names(code: &str) -> Vec<&'static str> {
    AIRPORTS
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, city, alts)| std::iter::once(*city).chain(alts.iter().copied()).collect())
        .unwrap_or_default()
}

/// Airport codes for a place named in a question ("lisbon", "lisboa",
/// "new york", "LIS"). Empty when unknown.
pub(crate) fn airports_for_place(place: &str) -> Vec<&'static str> {
    let p = crate::text::tokens(place)
        .into_iter()
        .map(|t| t.2)
        .collect::<Vec<_>>()
        .join(" ");
    if p.is_empty() {
        return vec![];
    }
    let fold = |s: &str| {
        crate::text::tokens(s)
            .into_iter()
            .map(|t| t.2)
            .collect::<Vec<_>>()
            .join(" ")
    };
    AIRPORTS
        .iter()
        .filter(|(code, city, alts)| {
            code.eq_ignore_ascii_case(&p) || fold(city) == p || alts.iter().any(|a| fold(a) == p)
        })
        .map(|(c, _, _)| *c)
        .collect()
}

/// The first known city named in `text` (a hotel's name or address):
/// (airport code, city). Only city names and multi-letter alternatives
/// count, not codes, so "LIS" in a booking reference isn't Lisbon.
pub(crate) fn city_in(text: &str) -> Option<(&'static str, &'static str)> {
    let fold = |s: &str| {
        crate::text::tokens(s)
            .into_iter()
            .map(|t| t.2)
            .collect::<Vec<_>>()
            .join(" ")
    };
    let hay = format!(" {} ", fold(text));
    AIRPORTS.iter().find_map(|(code, city, alts)| {
        std::iter::once(*city)
            .chain(alts.iter().copied())
            .filter(|n| n.len() > 3)
            .any(|n| hay.contains(&format!(" {} ", fold(n))))
            .then_some((*code, *city))
    })
}

fn contains_word(hay: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(p) = hay[from..].find(needle) {
        let s = from + p;
        let e = s + needle.len();
        let ok_before = hay[..s]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let ok_after = hay[e..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if ok_before && ok_after {
            return true;
        }
        from = e;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_resolve() {
        assert_eq!(airports_for_place("Lisbon"), vec!["LIS"]);
        assert_eq!(airports_for_place("lisboa"), vec!["LIS"]);
        assert_eq!(airports_for_place("LIS"), vec!["LIS"]);
        assert_eq!(airports_for_place("new york").len(), 3);
        assert!(airports_for_place("nowhere").is_empty());
        assert_eq!(airline_by_code("tp"), Some(("TP", "TAP Air Portugal")));
        assert_eq!(
            airline_in("thanks for flying tap air portugal"),
            Some(("TP", "TAP Air Portugal"))
        );
        // Codes are unique.
        let mut codes: Vec<&str> = AIRPORTS.iter().map(|a| a.0).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), AIRPORTS.len());
        let mut codes: Vec<&str> = AIRLINES.iter().map(|a| a.0).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), AIRLINES.len());
    }
}
