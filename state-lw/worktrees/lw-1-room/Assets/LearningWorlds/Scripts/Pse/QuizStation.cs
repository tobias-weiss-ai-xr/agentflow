using UnityEngine;

public class QuizStation : MonoBehaviour
{
    private Element selectedElement;
    private Question currentQuestion;
    private Transform answerPedestal;
    private Material correctGlow;
    private Material wrongOutline;

    public void Initialize(Element element)
    {
        selectedElement = element;
        this.name = "QuizStation" + element.Symbol;

        // Create answer pedestal
        GameObject pedestalRoot = new GameObject("AnswerPedestal");
        pedestalRoot.transform.parent = transform;
        answerPedestal = pedestalRoot.transform;

        // Create glow effects
        correctGlow = CreateMaterial(Color.green, 1f);
        wrongOutline = CreateMaterial(Color.red, 0.5f, (int)UnityEngine.Rendering.BlendMode.Once);

        // Create answer button
        GameObject answerButton = GameObject.CreatePrimitive(PrimitiveType.Cube);
        answerButton.transform.parent = answerPedestal;
        answerButton.transform.localPosition = Vector3.zero;
        answerButton.transform.localScale = new Vector3(0.5f, 0.05f, 0.5f);
       (answerButton.AddComponent<MeshFilter>().mesh, answerButton.AddComponent<MeshRenderer>().material) = (null,
            new Material(Shader.Find("Standard"))) {
                color = Color.gray
        };
        answerButton.AddComponent<QuizAnswerButton>().station = this;
    }

    public void GenerateNewQuestion()
    {
        string[] templates = {
            "{name}: {symbol}\nWhat is its {atomicNumber}?",
            "What is the symbol for {name}?",
            "{name} belongs to the group {category}, what kind of element is it?",
            "The melting point of {symbol} is: High/Medium/Low?",
            "{name} is often confused with {similarElementName}. What is its symbol?"
        };

        string questionPattern = templates[Random.Range(0, templates.Length)];
        string[] answers = GenerateAnswerChoices();

        currentQuestion = new Question() {
            prompt = string.Format(
                questionPattern, 
                selectedElement.Name, selectedElement.Symbol, selectedElement.AtomicNumber,
                selectedElement.Category, selectedElement.GetSimilarElementName()
            ),
            answer = answers[0],
            choices = answers
        };
    }

    private string[] GenerateAnswerChoices()
    {
        string wrongAnswer = selectedElement.Name;

        int index = Random.Range(0, Element.AllElements.Count * 2);
        if (index < Element.AllElements.Count)
        {
            wrongAnswer = Element.AllElements[index].Name;
        }
        else
        {
            // Use another random attribute
            int failedAttempt = 0;
            for (int i = 0; failedAttempt < 3 && i < Element.AllElements.Count; i++)
            {
                if (!string.IsNullOrEmpty(Element.AllElements[i].Symbol) && 
                    Element.AllElements[i].Symbol != selectedElement.Symbol)
                {
                    wrongAnswer = Element.AllElements[i].Name;
                    break;
                }
            }
        }

        List<string> choices = new List<string>() {selectedElement.Symbol};
        for (int i = 0; i < 3; i++)
        {
            choices.Add(wrongAnswer);
        }
        ShuffleList(choices);

        return choices.ToArray();
    }

    private System.Random rand = new System.Random();
    private void ShuffleList<T>( IList<T> list )
    {
        for (int i = 0; i < list.Count; i++)
        {
            T temp = list[i];
            int r = rand.Next(i, list.Count);
            list[i] = list[r];
            list[r] = temp;
        }
    }

    public void Answer(bool isCorrect)
    {
        answerPedestal.GetChild(0).GetComponent<Renderer>().material = isCorrect ? correctGlow : wrongOutline;

        // Visual feedback
        ColorMatChanger ch = answerPedestal.GetChild(0).GetComponent<ColorMatChanger>();
        if (ch == null)
        {
            ch = answerPedestal.GetChild(0).AddComponent<ColorMatChanger>();
        }
        ch.color = isCorrect ? Color.green : Color.red;
        ch.alertTime = 1.5f;
        Invoke("ResetFeedback", 1.5f);
    }

    private void ResetFeedback()
    {
        if (answerPedestal.childCount > 0)
        {
            answerPedestal.GetChild(0).GetComponent<Renderer>().material = wrongOutline;
            Component.Destroy(answerPedestal.GetChild(0).GetComponent<ColorMatChanger>());
        }
    }

    public class ColorMatChanger : MonoBehaviour
    {
        public Color color;
        public float alertTime = 1.5f;

        void Update()
        {
            if (alertTime > 0 && gameObject.GetComponent<Renderer>() != null)
            {
                Renderer rend = gameObject.GetComponent<Renderer>();
                if (rend.enabled)
                {
                    rend.material.color = color;
                }
                alertTime -= Time.deltaTime;
            }
        }
    }

    public class Question
    {
        public string prompt;
        public string answer;
        public string[] choices;
    }

    private Material CreateMaterial(Color color, float alpha = 1, int blend = (int)UnityEngine.Rendering.BlendMode.Alpha)
    {
        Material mat = new Material(Shader.Find("Standard"));
        mat.color = color;
        mat.SetInt("_SrcBlend", blend);
        mat.SetInt("_DstBlend", blend == (int)UnityEngine.Rendering.BlendMode.Once ? 
            (int)UnityEngine.Rendering.BlendMode.OneMinusSrcAlpha : (int)UnityEngine.Rendering.BlendMode.OneMinusSrcAlpha);
        return mat;
    }

    public class QuizAnswerButton : MonoBehaviour
    {
        public QuizStation station;

        void OnTriggerEnter(Collider other)
        {
            if (other.CompareTag("Player"))
            {
                station.Answer(true);
                Invoke("Last" + station.GenerateNewQuestion + 0.5f, true);
            }
        }
    }
}