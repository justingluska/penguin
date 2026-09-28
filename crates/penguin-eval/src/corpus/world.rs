//! The fictional cast: the mailbox owner, colleagues, friends, family,
//! clients and the companies that send them mail. All domains are
//! `.example` (RFC 2606).

use penguin_core::Address;

use super::addr;

pub const ME_NAME: &str = "Alex Moreno";

/// (address, nickname) per account: 0 = work, 1 = personal, 2 = freelance.
pub const ACCOUNTS: [(&str, &str); 3] = [
    ("alex.moreno@northwind.example", "work"),
    ("alexmoreno@mailbox.example", "personal"),
    ("hello@morenostudio.example", "studio"),
];
pub const WORK: usize = 0;
pub const PERSONAL: usize = 1;
pub const STUDIO: usize = 2;

/// A person: tag key, display name, address, role (for body text).
pub struct Person {
    pub key: &'static str,
    pub name: &'static str,
    pub email: &'static str,
    pub role: &'static str,
}

impl Person {
    pub fn addr(&self) -> Address {
        addr(self.name, self.email)
    }
    pub fn first(&self) -> &'static str {
        self.name.split(' ').next().unwrap()
    }
    pub fn tag(&self) -> String {
        format!("person:{}", self.key)
    }
}

macro_rules! people {
    ($name:ident: $( ($k:expr, $n:expr, $e:expr, $r:expr) ),* $(,)?) => {
        pub const $name: &[Person] = &[ $( Person { key: $k, name: $n, email: $e, role: $r } ),* ];
    };
}

// Colleagues at Northwind Labs, most frequent correspondents first.
people!(COLLEAGUES:
    ("priya", "Priya Shah", "priya.shah@northwind.example", "product manager"),
    ("mateo", "Mateo Ruiz", "mateo.ruiz@northwind.example", "backend engineer"),
    ("hannah", "Hannah Lindqvist", "hannah.lindqvist@northwind.example", "design lead"),
    ("kwame", "Kwame Mensah", "kwame.mensah@northwind.example", "finance"),
    ("yuki", "Yuki Tanaka", "yuki.tanaka@northwind.example", "security"),
    ("diego", "Diego Alvarez", "diego.alvarez@northwind.example", "sales"),
    ("lena", "Lena Fischer", "lena.fischer@northwind.example", "recruiting"),
    ("omar", "Omar Haddad", "omar.haddad@northwind.example", "CEO"),
    ("sam", "Sam Carter", "sam.carter@northwind.example", "operations"),
    ("nina", "Nina Petrova", "nina.petrova@northwind.example", "legal"),
    ("ben", "Ben Adeyemi", "ben.adeyemi@northwind.example", "data"),
    ("chloe", "Chloe Martin", "chloe.martin@northwind.example", "marketing"),
    ("mike-w", "Mike Chen", "mike.chen@northwind.example", "iOS engineer"),
);

// Outside the company: vendors, customers, partners.
people!(PARTNERS:
    ("theo", "Theo Laurent", "theo@crestline.example", "Crestline (vendor)"),
    ("ana", "Ana Sousa", "ana.sousa@lindenpartners.example", "Linden Partners (customer)"),
    ("ravi", "Ravi Nair", "ravi@bluepeak.example", "Bluepeak (customer)"),
    ("greta", "Greta Holm", "greta.holm@fjordsoft.example", "Fjordsoft (partner)"),
);

// Friends and family (personal account).
people!(FRIENDS:
    ("sofia", "Sofia Reyes", "sofia.reyes@mailbox.example", "partner"),
    ("jess", "Jess Park", "jesspark@inbox.example", "friend"),
    ("ben-o", "Ben Okafor", "ben.okafor@inbox.example", "friend"),
    ("luis", "Luis Moreno", "luis.moreno@mailbox.example", "brother"),
    ("carmen", "Carmen Moreno", "carmen.moreno@correo.example", "mother"),
    ("rosa", "Rosa Jiménez", "rosa.jimenez@correo.example", "aunt"),
    ("mike-d", "Mike Delgado", "mike.delgado@harborrealty.example", "landlord's agent"),
    ("dana", "Dana Whitfield", "dana.whitfield@inbox.example", "neighbour"),
    ("pablo", "Pablo Ortega", "pablo.ortega@correo.example", "old friend in Valencia"),
);

// Freelance clients (studio account).
people!(CLIENTS:
    ("julia", "Julia Brandt", "julia@fernhillbakery.example", "Fernhill Bakery"),
    ("marco", "Marco Bianchi", "marco@bianchiwines.example", "Bianchi Wines"),
    ("tom", "Tom Walsh", "tom@walshcycles.example", "Walsh Cycles"),
    ("irene", "Irene Castillo", "irene@casacastillo.example", "Casa Castillo (Madrid)"),
);

pub fn person(key: &str) -> &'static Person {
    COLLEAGUES
        .iter()
        .chain(PARTNERS)
        .chain(FRIENDS)
        .chain(CLIENTS)
        .find(|p| p.key == key)
        .unwrap_or_else(|| panic!("no person {key}"))
}

/// A company that sends automated mail.
pub struct Sender {
    pub key: &'static str,
    pub name: &'static str,
    pub email: &'static str,
}

impl Sender {
    pub fn addr(&self) -> Address {
        addr(self.name, self.email)
    }
    pub fn tag(&self) -> String {
        format!("merchant:{}", self.key)
    }
}

macro_rules! senders {
    ($name:ident: $( ($k:expr, $n:expr, $e:expr) ),* $(,)?) => {
        pub const $name: &[Sender] = &[ $( Sender { key: $k, name: $n, email: $e } ),* ];
    };
}

senders!(MERCHANTS:
    ("swiftcab", "Swiftcab", "receipts@swiftcab.example"),
    ("dishdash", "Dishdash", "orders@dishdash.example"),
    ("beanhouse", "Beanhouse Coffee", "hello@beanhouse.example"),
    ("parcelmart", "Parcelmart", "orders@parcelmart.example"),
    ("gearloft", "Gearloft", "orders@gearloft.example"),
    ("pagebound", "Pagebound Books", "orders@pagebound.example"),
    ("greengrocer", "Green Grocer", "receipts@greengrocer.example"),
    ("appmarket", "App Market", "no_reply@appmarket.example"),
    ("mercadosol", "Mercado Sol", "pedidos@mercadosol.example"),
);

senders!(BILLERS:
    ("cedarvalley", "Cedar Valley Power", "billing@cedarvalleypower.example"),
    ("brightwave", "Brightwave Internet", "billing@brightwave.example"),
    ("harbormobile", "Harbor Mobile", "billing@harbormobile.example"),
    ("pinecrest", "Pinecrest Water", "billing@pinecrestwater.example"),
    ("nimbus", "Nimbus Cloud", "billing@nimbuscloud.example"),
    ("streamly", "Streamly", "account@streamly.example"),
    ("tunewave", "Tunewave", "billing@tunewave.example"),
    ("shieldline", "Shieldline Insurance", "policies@shieldline.example"),
);

senders!(NEWSLETTERS:
    ("morningbyte", "The Morning Byte", "newsletter@morningbyte.example"),
    ("panpantry", "Pan & Pantry", "hello@panpantry.example"),
    ("strideweekly", "Stride Weekly", "news@strideweekly.example"),
    ("designnotes", "Design Notes", "issues@designnotes.example"),
    ("harborlocal", "Harbor Local News", "daily@harborlocal.example"),
    ("fieldguide", "The Field Guide", "letters@fieldguide.example"),
    ("elpulso", "El Pulso", "boletin@elpulso.example"),
);

senders!(PROMOS:
    ("gearloft", "Gearloft", "deals@gearloft.example"),
    ("parcelmart", "Parcelmart", "deals@parcelmart.example"),
    ("streamly", "Streamly", "hello@streamly.example"),
    ("dishdash", "Dishdash", "promos@dishdash.example"),
    ("skyward", "Skyward Air", "offers@skywardair.example"),
    ("pagebound", "Pagebound Books", "news@pagebound.example"),
);

senders!(AIRLINES:
    ("skyward", "Skyward Air", "reservations@skywardair.example"),
    ("pacifica", "Pacifica Airlines", "noreply@pacifica-air.example"),
    ("aerovia", "Aerovía", "reservas@aerovia.example"),
);

senders!(HOTELS:
    ("staywell", "Staywell Hotels", "reservations@staywell.example"),
    ("nestaway", "Nestaway", "bookings@nestaway.example"),
);

senders!(CARRIERS:
    ("parcelpost", "ParcelPost", "tracking@parcelpost.example"),
    ("swiftship", "SwiftShip", "updates@swiftship.example"),
);

senders!(CODE_SENDERS:
    ("ledgerly", "Ledgerly Bank", "security@ledgerly.example"),
    ("swiftcab", "Swiftcab", "no-reply@swiftcab.example"),
    ("parcelmart", "Parcelmart", "account@parcelmart.example"),
    ("nimbus", "Nimbus Cloud", "security@nimbuscloud.example"),
    ("circlely", "Circlely", "security@circlely.example"),
    ("solaris", "Banco Solaris", "seguridad@bancosolaris.example"),
    ("harbormobile", "Harbor Mobile", "verify@harbormobile.example"),
);

senders!(SOCIAL:
    ("circlely", "Circlely", "notifications@circlely.example"),
    ("makersforum", "Makers Forum", "digest@makersforum.example"),
    ("trailmates", "Trailmates", "notify@trailmates.example"),
);

/// Cities for trips: (key, name, airport code, country).
pub const CITIES: &[(&str, &str, &str, &str)] = &[
    ("lisbon", "Lisbon", "LIS", "Portugal"),
    ("madrid", "Madrid", "MAD", "Spain"),
    ("valencia", "Valencia", "VLC", "Spain"),
    ("chicago", "Chicago", "ORD", "USA"),
    ("denver", "Denver", "DEN", "USA"),
    ("seattle", "Seattle", "SEA", "USA"),
    ("austin", "Austin", "AUS", "USA"),
    ("boston", "Boston", "BOS", "USA"),
    ("toronto", "Toronto", "YYZ", "Canada"),
    ("mexico-city", "Mexico City", "MEX", "Mexico"),
    ("london", "London", "LHR", "UK"),
    ("tokyo", "Tokyo", "HND", "Japan"),
];
